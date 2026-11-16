use pavia_contract::Contract;

#[derive(Contract)]
struct Member {
  name: String,
  #[pavia(rename = "name")]
  nickname: String,
}

#[derive(Contract)]
enum Status {
  Online,
  #[pavia(rename = "Online")]
  Here,
}

fn main() {}
