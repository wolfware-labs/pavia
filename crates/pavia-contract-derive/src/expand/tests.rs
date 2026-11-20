use syn::{DeriveInput, parse_quote};

use super::contract;

#[test]
fn reaches_the_contract_crate_only_through_the_given_path() {
  let inputs: [DeriveInput; 4] = [
    parse_quote! {
      #[pavia(crate = "relay")]
      struct Page<T> {
        items: Vec<T>,
        #[pavia(optional)]
        next: Option<String>,
      }
    },
    parse_quote! {
      #[pavia(crate = "relay", open)]
      enum Event {
        Empty,
        Text(String),
        Pair(u32, u32),
        Moved { from: u32, to: u32 },
      }
    },
    parse_quote! {
      #[pavia(crate = "relay")]
      struct Pair(u32, String);
    },
    parse_quote! {
      #[pavia(crate = "relay")]
      struct Nothing;
    },
  ];
  for input in &inputs {
    let output = contract(input).unwrap().to_string();
    assert!(output.contains("relay :: Contract"), "{output}");
    assert!(!output.contains("pavia_contract"), "{output}");
  }
}
