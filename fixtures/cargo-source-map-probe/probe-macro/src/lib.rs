use proc_macro::TokenStream;
use std::env;
use std::fs;
use std::path::PathBuf;

fn compile_error(message: &str) -> TokenStream {
    format!("compile_error!({message:?});")
        .parse()
        .unwrap_or_default()
}

#[proc_macro]
pub fn observe_marker(_: TokenStream) -> TokenStream {
    let result = (|| -> Result<String, Box<dyn std::error::Error>> {
        let path = PathBuf::from(env::var("OUT_DIR")?).join("proc-macro-input.txt");
        let value = fs::read_to_string(&path)?;
        Ok(format!(
            "({:?}, {:?})",
            path.display().to_string(),
            value.trim()
        ))
    })();
    match result {
        Ok(expression) => expression.parse().unwrap_or_default(),
        Err(error) => compile_error(&format!(
            "could not observe generated proc-macro input: {error}"
        )),
    }
}
