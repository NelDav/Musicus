//! Assumption: freehand annotations stored as vector strokes in page-relative
//! coordinates stay small enough (a few KB per page) to save whole page layers
//! on every change and to keep them in SQLite instead of the blob store.

use musicus_proto::XorShift;
use serde::Serialize;

#[derive(Serialize)]
struct JsonStroke {
    tool: &'static str,
    color: String,
    width: f32,
    points: Vec<[f32; 3]>, // x, y in 0..1 of the page box, pressure 0..1
}

#[derive(Serialize)]
struct BinStroke {
    id: u64,
    tool: u8,
    color: u32,
    width: u16,
    points: Vec<[u16; 2]>, // quantized to 1/65535 of page width/height
    pressure: Vec<u8>,
}

#[derive(Serialize)]
struct DeltaStroke {
    id: u64,
    tool: u8,
    color: u32,
    width: u16,
    start: [u16; 2],
    deltas: Vec<[i16; 2]>, // postcard writes these as zigzag varints, mostly 1 byte
    pressure: Vec<u8>,
}

/// A pen stroke: a smooth random walk, ~1 point per 0.6 mm as a stylus at 120 Hz would produce.
fn stroke(rng: &mut XorShift, n: usize) -> Vec<[f32; 3]> {
    let (mut x, mut y) = (rng.unit(), rng.unit());
    let (mut dx, mut dy) = (0.002f32, 0.0f32);
    (0..n)
        .map(|_| {
            dx = (dx + (rng.unit() - 0.5) * 0.0015).clamp(-0.004, 0.004);
            dy = (dy + (rng.unit() - 0.5) * 0.0015).clamp(-0.004, 0.004);
            x = (x + dx).clamp(0.0, 1.0);
            y = (y + dy).clamp(0.0, 1.0);
            [x, y, 0.3 + rng.unit() * 0.7]
        })
        .collect()
}

fn q(v: f32) -> u16 {
    (v * 65535.0).round() as u16
}

fn main() {
    println!("{:<34} {:>9} {:>9} {:>9} {:>9} {:>9}", "page", "json", "json+zst", "postcard", "delta", "delta+zst");
    for (label, strokes, points) in [
        ("light: 8 strokes x 40 pts", 8, 40),
        ("typical: 30 strokes x 60 pts", 30, 60),
        ("heavy: 120 strokes x 80 pts", 120, 80),
    ] {
        let mut rng = XorShift::new(9);
        let raw: Vec<Vec<[f32; 3]>> = (0..strokes).map(|_| stroke(&mut rng, points)).collect();

        let json: Vec<JsonStroke> = raw
            .iter()
            .map(|p| JsonStroke { tool: "pen", color: "#d0021bff".into(), width: 0.8, points: p.clone() })
            .collect();
        let json = serde_json::to_vec(&json).unwrap();

        let bin: Vec<BinStroke> = raw
            .iter()
            .enumerate()
            .map(|(i, p)| BinStroke {
                id: i as u64,
                tool: 1,
                color: 0xd0021bff,
                width: 80,
                points: p.iter().map(|v| [q(v[0]), q(v[1])]).collect(),
                pressure: p.iter().map(|v| (v[2] * 255.0) as u8).collect(),
            })
            .collect();
        let postcard_plain = postcard::to_allocvec(&bin).unwrap();

        let delta: Vec<DeltaStroke> = bin
            .iter()
            .map(|s| DeltaStroke {
                id: s.id,
                tool: s.tool,
                color: s.color,
                width: s.width,
                start: s.points[0],
                deltas: s
                    .points
                    .windows(2)
                    .map(|w| [(w[1][0] as i32 - w[0][0] as i32) as i16, (w[1][1] as i32 - w[0][1] as i32) as i16])
                    .collect(),
                pressure: s.pressure.clone(),
            })
            .collect();
        let postcard_delta = postcard::to_allocvec(&delta).unwrap();

        let z = |b: &[u8]| zstd::encode_all(b, 3).unwrap().len();
        let kb = |n: usize| format!("{:.1} KB", n as f64 / 1000.0);
        println!(
            "{label:<34} {:>9} {:>9} {:>9} {:>9} {:>9}",
            kb(json.len()),
            kb(z(&json)),
            kb(postcard_plain.len()),
            kb(postcard_delta.len()),
            kb(z(&postcard_delta))
        );
    }
}
