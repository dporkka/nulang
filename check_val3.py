import struct
raw = 0x7FFC000000000000 | 0x7f435c1591eb
try:
    val = struct.unpack('d', struct.pack('Q', raw))[0]
    print(f"float: {val}")
except Exception as e:
    print(f"error: {e}")
