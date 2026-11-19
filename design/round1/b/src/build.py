#!/usr/bin/env python3
"""Собирает ../index.html: подставляет шрифты Jost и эмблему (data: URI) в index.src.html.
Источник: /home/save/Alexandrie/site (только чтение). Запуск: python3 build.py"""
import base64, re, io, pathlib
from PIL import Image
here = pathlib.Path(__file__).parent
site = pathlib.Path("/home/save/Alexandrie/site")
b64 = lambda b: base64.b64encode(b).decode()
fonts_css = (site / "fonts.css").read_text()
def ur(name):  # unicode-range строки Jost-<name> из fonts.css
    m = re.search(r'Jost-%s\.woff2\) format\("woff2"\); unicode-range: ([^;]+);' % name, fonts_css)
    return m.group(1)
big = Image.open(site / "assets/emblem-a.webp").convert("RGBA"); big.thumbnail((460, 500))
buf = io.BytesIO(); big.save(buf, "WEBP", quality=82)
rep = {
 "{{JOST_LATIN}}": b64((site/"assets/fonts/Jost-latin.woff2").read_bytes()),
 "{{JOST_CYR}}": b64((site/"assets/fonts/Jost-cyrillic.woff2").read_bytes()),
 "{{UR_LATIN}}": ur("latin"), "{{UR_CYR}}": ur("cyrillic"),
 "{{EMBLEM_SMALL}}": b64((site/"assets/emblem-a-small.webp").read_bytes()),
 "{{EMBLEM_BIG}}": b64(buf.getvalue()),
}
s = (here / "index.src.html").read_text()
for k, v in rep.items(): s = s.replace(k, v)
(here.parent / "index.html").write_text(s)
print("ok", len(s)//1024, "KB")
