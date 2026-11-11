//! The first of the two same-name derive macros (Relax KNOWN_GAPS #1 shape,
//! oxide#307). Emits `impl Who { pub fn who() -> &'static str { "a" } }`.

use proc_macro::TokenStream;

#[proc_macro_derive(Who)]
pub fn derive_who(_input: TokenStream) -> TokenStream {
    "impl Who { pub fn who() -> &'static str { \"a\" } }"
        .parse()
        .expect("emit impl Who for mac-a")
}
