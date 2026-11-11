//! The second same-name derive macro (Relax KNOWN_GAPS #1 shape, oxide#307).
//! Emits `impl Who { pub fn who() -> &'static str { "b" } }`.

use proc_macro::TokenStream;

#[proc_macro_derive(Who)]
pub fn derive_who(_input: TokenStream) -> TokenStream {
    "impl Who { pub fn who() -> &'static str { \"b\" } }"
        .parse()
        .expect("emit impl Who for mac-b")
}
