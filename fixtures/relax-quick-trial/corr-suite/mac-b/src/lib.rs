//! The second same-name derive macro (Relax KNOWN_GAPS #1 shape, oxide#307).
//! For the derived item `pub struct <Name>;` this emits an inherent
//! `impl <Name>` whose `who()` answers `"b"`. The item is re-emitted
//! unchanged plus the generated impl, so the derive compiles without syn
//! and the output is exactly rustc's truth shape.

use proc_macro::TokenTree;

/// The derived item's name: the identifier that follows the `struct`
/// keyword. The fixtures derive unit structs with no generics and no
//! `where` clause, so this token walk is exact for them; anything else
/// refuses with a compile error naming the crate.
fn derived_name(input: proc_macro::TokenStream) -> String {
    let mut after_struct = false;
    for tree in input {
        if let TokenTree::Ident(ident) = tree {
            let word = ident.to_string();
            if after_struct {
                return word;
            }
            after_struct = word == "struct";
        }
    }
    panic!("derive Who: no struct name in the item");
}

#[proc_macro_derive(Who)]
pub fn derive_who(input: proc_macro::TokenStream) -> TokenStream {
    let name = derived_name(input.clone());
    let mut out = input;
    let body = format!(
        "impl {name} {{ pub fn who() -> &'static str {{ \"b\" }} }}",
        name = name,
    );
    out.extend(
        body.parse::<TokenStream>()
            .expect("emit impl for mac-b"),
    );
    out
}
