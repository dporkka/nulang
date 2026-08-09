fn main() {
    let tag_ptr = 0x7FFC_0000_0000_0000u64;
    let ptr = 139927284453867u64;
    let raw = tag_ptr | ptr;
    println!("raw hex: {:x}", raw);
    
    // Simulate is_float_raw
    let EXPONENT_MASK = 0x7FF0_0000_0000_0000u64;
    let MANTISSA_MASK = 0x000F_FFFF_FFFF_FFFFu64;
    let is_float = (raw & EXPONENT_MASK) != EXPONENT_MASK || (raw & MANTISSA_MASK) == 0;
    println!("is_float: {}", is_float);
    
    // Simulate as_float and f.to_string()
    if is_float {
        let f = f64::from_bits(raw);
        println!("float string: {}", f);
    }
}
