use std::net::SocketAddr;

use hyper::HeaderMap;
use hyper::header::{self, HeaderValue};
use pavia_proto::frame::{Auth, Token};

use super::{Authenticator, Permissive, Principal, RequestContext};

fn context(cookies: &[&'static str]) -> RequestContext {
  let mut headers = HeaderMap::new();
  headers.insert("x-pavia-test", HeaderValue::from_static("yes"));
  for &cookie in cookies {
    headers.append(header::COOKIE, HeaderValue::from_static(cookie));
  }
  RequestContext::new(headers, Some(SocketAddr::from(([192, 0, 2, 7], 40_000))))
}

#[test]
fn compares_principals_by_stable_id() {
  let equal = [
    (Principal::for_user("u-42"), Principal::for_user("u-42")),
    (
      Principal::for_user("u-1").with_id("key-7"),
      Principal::for_user("u-2").with_id("key-7"),
    ),
    (Principal::anonymous(), Principal::anonymous()),
  ];
  for (left, right) in equal {
    assert_eq!(left, right);
  }
  let different = [
    (Principal::for_user("u-42"), Principal::for_user("u-43")),
    (Principal::for_user("u-42"), Principal::anonymous()),
    (
      Principal::for_user("u-42").with_id("key-7"),
      Principal::anonymous().with_id("key-7"),
    ),
  ];
  for (left, right) in different {
    assert_ne!(left, right);
  }
}

#[test]
fn reports_an_anonymous_principal() {
  let anonymous = Principal::anonymous();
  assert!(anonymous.is_anonymous());
  assert_eq!((anonymous.id(), anonymous.user()), (None, None));
  let user = Principal::for_user("u-42").with_id("key-7");
  assert!(!user.is_anonymous());
  assert_eq!((user.id(), user.user()), (Some("key-7"), Some("u-42")));
}

#[test]
fn reads_headers_cookies_and_the_peer() {
  let context = context(&["theme=dark; session=c00kie", "flag=x=y"]);
  assert_eq!(context.header("X-Pavia-Test"), Some(&b"yes"[..]));
  assert_eq!(context.header("x-missing"), None);
  assert_eq!(context.header("not a header name"), None);
  assert_eq!(context.cookie("session"), Some("c00kie"));
  assert_eq!(context.cookie("theme"), Some("dark"));
  assert_eq!(context.cookie("flag"), Some("x=y"));
  assert_eq!(context.cookie("sess"), None);
  assert_eq!(context.peer(), Some(SocketAddr::from(([192, 0, 2, 7], 40_000))));
}

#[test]
fn never_prints_credentials() {
  let printed = format!("{:?}", context(&["session=c00kie"]));
  assert!(printed.contains("cookie") && printed.contains("x-pavia-test"), "{printed}");
  assert!(!printed.contains("c00kie") && !printed.contains("yes"), "{printed}");
  let auth = Auth {
    scheme: "bearer".into(),
    token: Token::Text("t0ken-value".into()),
  };
  assert!(!format!("{auth:?}").contains("t0ken-value"));
}

#[tokio::test]
async fn accepts_everyone_as_anonymous_with_the_permissive_authenticator() {
  let auth = Auth {
    scheme: "bearer".into(),
    token: Token::Text("anything".into()),
  };
  for auth in [None, Some(&auth)] {
    let principal = Permissive.authenticate(auth, &context(&[])).await;
    assert_eq!(principal.map(|principal| principal.is_anonymous()), Ok(true));
  }
  assert!(Permissive.schemes().is_empty());
}
