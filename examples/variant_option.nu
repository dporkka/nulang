// User-declared variant types — an Option-like type with construction
// and pattern matching (SPEC2 §3.4.1). Nulang has no prelude: programs
// declare the variants they need.
//
// Run with: nulang examples/variant_option.nu

type Option[T] = Some(T) | None

fn unwrap_or(o: Option[Int], default: Int) -> Int {
    match o with {
        | Some(x) => x
        | None => default
    }
}

let present = Some(41) in
let absent = None in
unwrap_or(present, 0) + unwrap_or(absent, 1)
