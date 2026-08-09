import struct
raw = 139927284453867
print(f"hex: {hex(raw)}")
# is it a float?
def is_float_raw(raw):
    # EXPONENT_MASK = 0x7FF0_0000_0000_0000
    # MANTISSA_MASK = 0x000F_FFFF_FFFF_FFFF
    EXPONENT_MASK = 0x7FF0000000000000
    MANTISSA_MASK = 0x000FFFFFFFFFFFFF
    return (raw & EXPONENT_MASK) != EXPONENT_MASK or (raw & MANTISSA_MASK) == 0

print(f"is_float: {is_float_raw(raw)}")

def to_string_repr(raw):
    # if it's float
    if is_float_raw(raw):
        # convert to f64
        return struct.unpack('d', struct.pack('Q', raw))[0]
    return "not float"

print(f"repr: {to_string_repr(raw)}")
