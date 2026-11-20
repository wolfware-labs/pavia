use pavia_contract::Contract;

#[derive(Contract)]
enum Status {
  #[pavia(rename = "foo-bar")]
  FooBar,
}

fn main() {}
