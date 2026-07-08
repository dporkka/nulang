// Algebraic effects — a handler intercepts a performed effect and
// resumes the computation with a value.
//
// Run with: nulang examples/effects.nu

handle perform Math.getAnswer() {
    | Math.getAnswer() => 42
}
