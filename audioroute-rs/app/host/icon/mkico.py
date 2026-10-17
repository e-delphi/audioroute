# Junta os PNGs i16..i256 de uma pasta num .ico (sem bibliotecas). Ver gerar.ps1.
import struct, sys, zlib
src, out = sys.argv[1], sys.argv[2]
sizes = [16, 24, 32, 48, 64, 128, 256]

def corner_alpha(png):
    # Lê o PNG (RGBA 8 bits) só para conferir a transparência do canto.
    pos, idat, w = 8, b'', 0
    while pos < len(png):
        n, kind = struct.unpack('>I4s', png[pos:pos + 8])
        data = png[pos + 8:pos + 8 + n]
        if kind == b'IHDR':
            w, h, depth, ctype = struct.unpack('>IIBB', data[:10])
            assert depth == 8 and ctype == 6, (depth, ctype)
        elif kind == b'IDAT':
            idat += data
        pos += 12 + n
    raw = zlib.decompress(idat)
    return raw[1 + 3]  # 1ª linha: byte de filtro + RGBA do 1º pixel

pngs = [open(f'{src}/i{s}.png', 'rb').read() for s in sizes]
print('alfa do canto:', [corner_alpha(p) for p in pngs])
header = struct.pack('<HHH', 0, 1, len(sizes))
offset = 6 + 16 * len(sizes)
entries, blobs = b'', b''
for s, png in zip(sizes, pngs):
    entries += struct.pack('<BBBBHHII', s % 256, s % 256, 0, 0, 1, 32, len(png), offset + len(blobs))
    blobs += png
open(out, 'wb').write(header + entries + blobs)
print('ico:', len(header + entries + blobs), 'bytes')
