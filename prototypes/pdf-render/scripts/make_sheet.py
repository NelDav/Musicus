"""Generates synthetic test parts: a vector-engraved one (like Sibelius/MuseScore
exports) and a scanned one (300 dpi bitmap per page, like a digitised library).
No copyrighted material involved."""
import random, sys, io
from reportlab.lib.pagesizes import A4
from reportlab.pdfgen import canvas

def engraved(path, pages=4):
    random.seed(1)
    c = canvas.Canvas(path, pagesize=A4)
    w, h = A4
    for p in range(pages):
        c.setFont("Times-Bold", 16); c.drawCentredString(w/2, h-40, "Test March - 2nd Trumpet in Bb")
        y = h - 90
        for staff in range(11):
            c.setLineWidth(0.5)
            for l in range(5):
                c.line(40, y - l*5, w-40, y - l*5)
            x = 60
            while x < w - 50:
                for _ in range(4):  # notes in a bar
                    ny = y - random.randint(-2, 10) * 2.5
                    c.saveState(); c.translate(x, ny); c.rotate(20)
                    c.ellipse(-3.2, -2.2, 3.2, 2.2, fill=1); c.restoreState()
                    c.setLineWidth(0.8); c.line(x+3, ny, x+3, ny+17)
                    if random.random() < 0.3:
                        c.setFont("Times-Italic", 9); c.drawString(x-4, y-32, random.choice(["p","mf","f","cresc."]))
                    x += 14
                c.setLineWidth(0.6); c.line(x-4, y, x-4, y-20); x += 8
            y -= 68
        c.setFont("Times-Roman", 9); c.drawCentredString(w/2, 20, f"- {p+1} -")
        c.showPage()
    c.save()

def scanned(path, pages=4):
    from PIL import Image, ImageDraw, ImageFilter
    from reportlab.lib.utils import ImageReader
    c = canvas.Canvas(path, pagesize=A4)
    w, h = A4
    random.seed(2)
    for p in range(pages):
        img = Image.new("L", (2480, 3508), 245)  # A4 at 300 dpi, slightly grey paper
        d = ImageDraw.Draw(img)
        y = 300
        for staff in range(11):
            for l in range(5):
                d.line([(150, y+l*21), (2330, y+l*21)], fill=20, width=3)
            x = 220
            while x < 2300:
                ny = y + random.randint(-2, 10) * 10
                d.ellipse([x-13, ny-9, x+13, ny+9], fill=10); d.line([(x+12, ny), (x+12, ny-70)], fill=10, width=4)
                x += 58
            y += 285
        img = img.filter(ImageFilter.GaussianBlur(0.8))
        noise = Image.effect_noise(img.size, 12)
        img = Image.blend(img, noise, 0.08)
        buf = io.BytesIO(); img.save(buf, format="JPEG", quality=75); buf.seek(0)
        c.drawImage(ImageReader(buf), 0, 0, width=w, height=h)
        c.showPage()
    c.save()

engraved(sys.argv[1] + "/engraved.pdf")
scanned(sys.argv[1] + "/scanned.pdf")
