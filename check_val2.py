def is_float_raw(raw):
    EXPONENT_MASK = 0x7FF0000000000000
    MANTISSA_MASK = 0x000FFFFFFFFFFFFF
    return (raw & EXPONENT_MASK) != EXPONENT_MASK or (raw & MANTISSA_MASK) == 0

ptr = 139927284453867
raw = 0x7FFC000000000000 | ptr
print(f"hex raw: {hex(raw)}")
print(f"is_float: {is_float_raw(raw)}")
