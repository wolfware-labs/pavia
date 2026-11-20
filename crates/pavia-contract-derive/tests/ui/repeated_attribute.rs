use pavia_contract::Contract;

#[derive(Contract)]
struct Member {
  #[pavia(rename = "handle", rename = "nick")]
  nickname: String,
}

#[derive(Contract)]
#[pavia(open)]
#[pavia(open)]
enum Status {
  Online,
}

#[derive(Contract)]
struct Settings {
  #[pavia(optional, optional)]
  theme: Option<String>,
}

fn main() {}
