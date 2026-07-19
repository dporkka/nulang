import Lake
open Lake DSL

package «nulang-formal» {}

@[default_target]
lean_lib «Nulang» {
  srcDir := "."
  roots := #[`types, `capabilities, `effects]
}
