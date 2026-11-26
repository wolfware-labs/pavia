//! The TypeScript back end (§12.4): one module for the `@pavia/client` runtime, holding the contract
//! fingerprint, a type per named type, a client class per service with server methods, a typed error
//! class per method with an error type, and a handler interface per service with client methods.
//!
//! Names are the manifest's as they are; class names are derived from them (`chat` gives `ChatClient`,
//! `chat/send` gives `ChatSendError`). A name the TypeScript code cannot carry, a keyword, a global the
//! generated code uses, or a member every object has, fails generation instead of being renamed.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use pavia_contract::manifest::{Call, Manifest, Method, Param, Service, Shape};
use pavia_contract::schema::{Definition, Enum, Field, Primitive, Struct, Type};
use serde_json::Value;

use crate::GenerateError;
use crate::front::{Body, Form, Front, Names, body, form, json, pascal, unsupported, walk};

/// `writeln!` into a `String`, which cannot fail.
macro_rules! emit {
  ($out:expr, $($format:tt)*) => {
    writeln!($out, $($format)*).expect("writing to a String")
  };
}

const TARGET: &str = "TypeScript";

/// The line length an import fits on before it takes one name per line.
const WIDTH: usize = 120;

const KEYWORDS: &[&str] = &[
  "abstract",
  "any",
  "as",
  "asserts",
  "async",
  "await",
  "bigint",
  "boolean",
  "break",
  "case",
  "catch",
  "class",
  "const",
  "constructor",
  "continue",
  "debugger",
  "declare",
  "default",
  "delete",
  "do",
  "else",
  "enum",
  "export",
  "extends",
  "false",
  "finally",
  "for",
  "from",
  "function",
  "get",
  "if",
  "implements",
  "import",
  "in",
  "infer",
  "instanceof",
  "interface",
  "intrinsic",
  "is",
  "keyof",
  "let",
  "module",
  "namespace",
  "never",
  "new",
  "null",
  "number",
  "object",
  "of",
  "out",
  "override",
  "package",
  "private",
  "protected",
  "public",
  "readonly",
  "require",
  "return",
  "satisfies",
  "set",
  "static",
  "string",
  "super",
  "switch",
  "symbol",
  "this",
  "throw",
  "true",
  "try",
  "type",
  "typeof",
  "undefined",
  "unique",
  "unknown",
  "var",
  "void",
  "while",
  "with",
  "yield",
];

const GLOBALS: &[&str] = &[
  "Array",
  "AsyncIterable",
  "BigInt",
  "Boolean",
  "Date",
  "Error",
  "Function",
  "Map",
  "Number",
  "Object",
  "Promise",
  "Readonly",
  "Record",
  "String",
  "Symbol",
  "Uint8Array",
];

const RUNTIME: &[&str] = &[
  "CallOptions",
  "CallSignature",
  "Connection",
  "NotifyOptions",
  "NotifySignature",
  "StatusError",
  "Types",
];

const CONSTANTS: &[&str] = &["CALLS", "CONTRACT", "NOTIFICATIONS", "TYPES"];

/// Names a type, a method or a field cannot take: members every object has, `prototype`, and `then`,
/// which makes an object a thenable that `await` calls. A type named `__proto__` would set the
/// prototype of the types table instead of adding its entry.
const MEMBERS: &[&str] = &[
  "__proto__",
  "constructor",
  "hasOwnProperty",
  "isPrototypeOf",
  "propertyIsEnumerable",
  "prototype",
  "then",
  "toLocaleString",
  "toString",
  "valueOf",
];

/// The type of a struct without fields: an object without keys. TypeScript's `{}` would take any
/// value but null and undefined, `0` and `""` included.
const EMPTY: &str = "Record<string, never>";

const LOCALS: &[&str] = &["arguments", "error", "eval", "items", "options"];

/// The TypeScript module for `manifest`, for a file next to the application's code.
///
/// # Errors
///
/// [`GenerateError::Invalid`] for a manifest [`Manifest::validate`] refuses,
/// [`GenerateError::Reserved`] for a name TypeScript or the generated code reserves, and
/// [`GenerateError::Collision`] for two parts that would get one name.
pub fn generate(manifest: &Manifest) -> Result<String, GenerateError> {
  let front = Front::new(manifest)?;
  check_option_cycles(manifest)?;
  let names = claim_names(&front)?;
  check_members(manifest, &names)?;
  Writer::new(&front).module()
}

fn reserved(what: impl Into<String>, name: &str) -> GenerateError {
  GenerateError::Reserved {
    what: what.into(),
    name: name.to_owned(),
    target: TARGET,
  }
}

fn claim_names(front: &Front<'_>) -> Result<Names, GenerateError> {
  let mut names = Names::default();
  for name in RUNTIME.iter().chain(CONSTANTS).chain(GLOBALS) {
    names.claim(name, "the generated code")?;
  }
  for name in front.manifest.type_table().keys() {
    if KEYWORDS.contains(&name.as_str()) || MEMBERS.contains(&name.as_str()) || names.contains(name) {
      return Err(reserved("type", name));
    }
    names.claim(name, format!("type `{name}`"))?;
  }
  for name in front.manifest.type_table().keys().filter(|name| front.has_input_form(name)) {
    names.claim(&input_name(name), format!("the input form of type `{name}`"))?;
  }
  for service in front.manifest.services() {
    if has_client_class(service) {
      names.claim(&client_class(service), format!("the client of service `{}`", service.name()))?;
    }
    if has_handlers_interface(service) {
      names.claim(
        &handlers_interface(service),
        format!("the handlers of service `{}`", service.name()),
      )?;
    }
    for method in service.server_methods().iter().filter(|method| has_error_class(method)) {
      names.claim(
        &error_class(service, method),
        format!("the error of `{}`", method_path(service, method)),
      )?;
    }
  }
  Ok(names)
}

/// Refuses a named type that is an option of itself: `A = option<A>`, or a longer loop of aliases
/// with an option on it, gives a TypeScript alias that refers to itself. Each alias leads to at most
/// one other through its options, so one walk along each chain finds every loop.
fn check_option_cycles(manifest: &Manifest) -> Result<(), GenerateError> {
  let table = manifest.type_table();
  let mut done = BTreeSet::new();
  for start in table.keys() {
    let mut path = BTreeSet::new();
    let mut current = Some(start.as_str());
    while let Some(name) = current.filter(|name| !done.contains(name)) {
      if !path.insert(name) {
        return Err(GenerateError::OptionCycle {
          name: name.to_owned(),
          target: TARGET,
        });
      }
      current = match table.get(name) {
        Some(definition) => optional_alias_of(definition)?,
        None => None,
      };
    }
    done.extend(path);
  }
  Ok(())
}

/// The named type an alias stands for once its options are taken off: `option<B>` gives `B`.
fn optional_alias_of(definition: &Definition) -> Result<Option<&str>, GenerateError> {
  let Body::Alias(mut ty) = body(definition)? else {
    return Ok(None);
  };
  while let Form::Option(inner) = form(ty)? {
    ty = inner;
  }
  Ok(match form(ty)? {
    Form::Ref(name) => Some(name),
    _ => None,
  })
}

/// Whether `ty` is `unit`, which a callee answers with as `void`.
fn is_unit(ty: &Type) -> Result<bool, GenerateError> {
  Ok(matches!(form(ty)?, Form::Primitive(Primitive::Unit)))
}

fn check_fields(fields: &Struct, what: &str) -> Result<(), GenerateError> {
  match fields.fields().iter().find(|field| MEMBERS.contains(&field.name())) {
    Some(field) => Err(reserved(format!("field of {what}"), field.name())),
    None => Ok(()),
  }
}

/// Checks the fields of every struct `ty` holds, named types' fields aside.
fn check_inline_fields(ty: &Type, what: &str) -> Result<(), GenerateError> {
  walk(ty, &mut |form| match form {
    Form::Struct(fields) => check_fields(fields, what),
    _ => Ok(()),
  })
}

fn check_members(manifest: &Manifest, names: &Names) -> Result<(), GenerateError> {
  for (name, definition) in manifest.type_table() {
    let what = format!("type `{name}`");
    let body = body(definition)?;
    if let Body::Struct(fields) = body {
      check_fields(fields, &what)?;
    }
    for ty in body.types() {
      check_inline_fields(ty, &what)?;
    }
  }
  for service in manifest.services() {
    for (side, methods) in [("server", service.server_methods()), ("client", service.client_methods())] {
      for method in methods {
        let what = format!("{side} method of service `{}`", service.name());
        if MEMBERS.contains(&method.name()) {
          return Err(reserved(what, method.name()));
        }
        let place = format!("{what} `{}`", method.name());
        let call_types = method
          .as_call()
          .map(|call| [call.accepts(), call.yields(), call.returns(), call.error_type()]);
        for ty in method
          .params()
          .iter()
          .map(Param::ty)
          .chain(call_types.into_iter().flatten().flatten())
        {
          check_inline_fields(ty, &place)?;
        }
        for param in method.params() {
          let name = param.name();
          if KEYWORDS.contains(&name) || LOCALS.contains(&name) || names.contains(name) {
            return Err(reserved(format!("param of {what} `{}`", method.name()), name));
          }
        }
      }
    }
  }
  Ok(())
}

fn has_error_class(method: &Method) -> bool {
  method
    .as_call()
    .is_some_and(|call| call.shape() == Shape::Unary && call.error_type().is_some())
}

/// The method's path (§8.1): `chat/send`.
fn method_path(service: &Service, method: &Method) -> String {
  format!("{}/{}", service.name(), method.name())
}

/// Whether `service` gets a client class: it has server methods, so the class reads its connection.
fn has_client_class(service: &Service) -> bool {
  !service.server_methods().is_empty()
}

/// Whether `service` gets a handler interface: it has client methods.
fn has_handlers_interface(service: &Service) -> bool {
  !service.client_methods().is_empty()
}

/// The client class of `service`: `chat` gives `ChatClient`.
fn client_class(service: &Service) -> String {
  identifier(format!("{}Client", pascal(service.name())))
}

/// The handler interface of `service`: `chat` gives `ChatHandlers`.
fn handlers_interface(service: &Service) -> String {
  identifier(format!("{}Handlers", pascal(service.name())))
}

/// The error class of a unary method with an error type: `chat/send` gives `ChatSendError`.
fn error_class(service: &Service, method: &Method) -> String {
  identifier(format!("{}{}Error", pascal(service.name()), pascal(method.name())))
}

/// `name` with an underscore before a leading digit, which [`pascal`] leaves when it drops the
/// underscores of a name such as `_2fa`.
fn identifier(name: String) -> String {
  if name.starts_with(|first: char| first.is_ascii_digit()) {
    format!("_{name}")
  } else {
    name
  }
}

/// The name of a handler method in its interface: `new` quoted, since `new(...)` there declares a
/// construct signature.
fn handler_member(method: &Method) -> String {
  match method.name() {
    "new" => quoted("new"),
    name => name.to_owned(),
  }
}

/// The input form of the named type `name`: `Status` gives `StatusInput`.
fn input_name(name: &str) -> String {
  format!("{name}Input")
}

fn quoted(text: &str) -> String {
  json(&Value::from(text))
}

/// The lines of `text` for a block comment: split at every line terminator JavaScript knows, with
/// `*/` broken so the text cannot end the comment.
fn comment_lines(text: &str) -> Vec<String> {
  text
    .split("\r\n")
    .flat_map(|line| line.split(['\r', '\n', '\u{2028}', '\u{2029}']))
    .map(|line| line.replace("*/", "*\\/"))
    .collect()
}

/// The parts of a proxy method's call: its parameter list, its signature's key and its arguments.
struct Invocation<'a> {
  params: &'a str,
  key: &'a str,
  args: &'a str,
}

struct Writer<'f, 'm> {
  front: &'f Front<'m>,
  out: String,
}

impl<'f, 'm> Writer<'f, 'm> {
  const fn new(front: &'f Front<'m>) -> Self {
    Self {
      front,
      out: String::new(),
    }
  }

  fn manifest(&self) -> &'m Manifest {
    self.front.manifest
  }

  fn unary(&self) -> Vec<(&'m Service, &'m Method, &'m Call)> {
    self
      .manifest()
      .services()
      .iter()
      .flat_map(|service| service.server_methods().iter().map(move |method| (service, method)))
      .filter_map(|(service, method)| method.as_call().map(|call| (service, method, call)))
      .filter(|(_, _, call)| call.shape() == Shape::Unary)
      .collect()
  }

  fn notifications(&self) -> Vec<(&'m Service, &'m Method)> {
    self
      .manifest()
      .services()
      .iter()
      .flat_map(|service| service.server_methods().iter().map(move |method| (service, method)))
      .filter(|(_, method)| method.as_call().is_none())
      .collect()
  }

  fn module(mut self) -> Result<String, GenerateError> {
    self.out.push_str("// Generated by `pavia codegen ts`. Do not edit.\n");
    self.imports();
    self.out.push('\n');
    self.doc("", self.manifest().documentation());
    let rows: Vec<String> = self
      .front
      .fingerprint
      .chunks(16)
      .map(|row| row.iter().map(u8::to_string).collect::<Vec<_>>().join(", "))
      .collect();
    emit!(
      self.out,
      "export const CONTRACT: Uint8Array = Uint8Array.from([\n  {},\n]);",
      rows.join(",\n  ")
    );
    for (name, definition) in self.manifest().type_table() {
      self.out.push('\n');
      let definition = body(definition)?;
      self.definition(name, &definition, false)?;
      if self.front.has_input_form(name) {
        self.out.push('\n');
        self.definition(name, &definition, true)?;
      }
    }
    self.signatures();
    for (service, method, call) in self.unary() {
      if let Some(error) = call.error_type() {
        self.out.push('\n');
        self.error_type_class(service, method, error)?;
      }
    }
    for service in self.manifest().services() {
      if has_client_class(service) {
        self.out.push('\n');
        self.client(service)?;
      }
      if has_handlers_interface(service) {
        self.out.push('\n');
        self.handlers(service)?;
      }
    }
    Ok(self.out)
  }

  fn imports(&mut self) {
    let services = self.manifest().services();
    let calls = services
      .iter()
      .flat_map(Service::server_methods)
      .any(|method| method.as_call().is_some());
    let unary = self.unary();
    let errors = unary.iter().any(|(_, _, call)| call.error_type().is_some());
    let unary = !unary.is_empty();
    let notifications = !self.notifications().is_empty();
    let used = [
      ("type CallOptions", calls),
      ("type CallSignature", unary),
      ("type Connection", services.iter().any(has_client_class)),
      ("type NotifyOptions", notifications),
      ("type NotifySignature", notifications),
      ("StatusError", errors),
      ("type Types", unary || notifications),
    ];
    let names: Vec<&str> = used.iter().filter(|(_, used)| *used).map(|(name, _)| *name).collect();
    if names.is_empty() {
      return;
    }
    let line = format!("import {{ {} }} from \"@pavia/client\";", names.join(", "));
    if line.len() <= WIDTH {
      emit!(self.out, "{line}");
    } else {
      emit!(self.out, "import {{\n  {},\n}} from \"@pavia/client\";", names.join(",\n  "));
    }
  }

  fn doc(&mut self, indent: &str, text: Option<&str>) {
    let Some(text) = text else { return };
    emit!(self.out, "{indent}/**");
    for line in comment_lines(text) {
      if line.is_empty() {
        emit!(self.out, "{indent} *");
      } else {
        emit!(self.out, "{indent} * {line}");
      }
    }
    emit!(self.out, "{indent} */");
  }

  fn definition(&mut self, name: &str, definition: &Body<'_>, input: bool) -> Result<(), GenerateError> {
    let declared = if input { input_name(name) } else { name.to_owned() };
    match definition {
      Body::Struct(fields) if fields.fields().is_empty() => {
        emit!(self.out, "export type {declared} = {EMPTY};");
      }
      Body::Struct(fields) => {
        emit!(self.out, "export interface {declared} {{");
        for field in fields.fields() {
          let field = self.field(field, input)?;
          emit!(self.out, "  {field};");
        }
        self.out.push_str("}\n");
      }
      Body::Enum(variants) => {
        let union = self.variants(variants, input)?;
        emit!(self.out, "export type {declared} = {union};");
      }
      Body::Alias(target) => {
        let ty = self.ty(target, input)?;
        emit!(self.out, "export type {declared} = {ty};");
      }
    }
    Ok(())
  }

  fn field(&self, field: &Field, input: bool) -> Result<String, GenerateError> {
    let mark = if field.is_optional() { "?" } else { "" };
    Ok(format!("{}{mark}: {}", field.name(), self.ty(field.ty(), input)?))
  }

  fn variants(&self, variants: &Enum, input: bool) -> Result<String, GenerateError> {
    let mut members = variants
      .variants()
      .iter()
      .map(|variant| match variant.ty() {
        None => Ok(format!("{{ kind: {} }}", quoted(variant.name()))),
        Some(ty) => Ok(format!(
          "{{ kind: {}; value: {} }}",
          quoted(variant.name()),
          self.ty(ty, input)?
        )),
      })
      .collect::<Result<Vec<String>, GenerateError>>()?;
    if variants.is_open() && !input {
      members.push("{ kind: \"Unknown\"; name: string }".to_owned());
    }
    Ok(if members.is_empty() {
      "never".to_owned()
    } else {
      members.join(" | ")
    })
  }

  /// The TypeScript type of the values of `ty`: as decoded, or, with `input`, as a sender writes them.
  ///
  /// # Errors
  ///
  /// [`GenerateError::Unsupported`] for a form or a primitive this back end does not know.
  fn ty(&self, ty: &Type, input: bool) -> Result<String, GenerateError> {
    Ok(match form(ty)? {
      Form::Primitive(primitive) => primitive_ty(primitive)?.to_owned(),
      Form::Option(inner) => format!("{} | null", self.ty(inner, input)?),
      Form::List(inner) => {
        let item = self.ty(inner, input)?;
        if matches!(form(inner)?, Form::Option(_)) {
          format!("({item})[]")
        } else {
          format!("{item}[]")
        }
      }
      Form::Map(inner) => format!("Map<string, {}>", self.ty(inner, input)?),
      Form::Tuple(items) => {
        let items = items
          .iter()
          .map(|item| self.ty(item, input))
          .collect::<Result<Vec<String>, GenerateError>>()?;
        format!("[{}]", items.join(", "))
      }
      Form::Struct(fields) => self.inline_struct(fields, input)?,
      Form::Ref(name) if input && self.front.has_input_form(name) => input_name(name),
      Form::Ref(name) => name.to_owned(),
    })
  }

  fn inline_struct(&self, fields: &Struct, input: bool) -> Result<String, GenerateError> {
    if fields.fields().is_empty() {
      return Ok(EMPTY.to_owned());
    }
    let fields = fields
      .fields()
      .iter()
      .map(|field| self.field(field, input))
      .collect::<Result<Vec<String>, GenerateError>>()?;
    Ok(format!("{{ {} }}", fields.join("; ")))
  }

  /// The type of a value a callee answers with: `void` for `unit`, and the form `input` asks for.
  fn result(&self, ty: &Type, input: bool) -> Result<String, GenerateError> {
    if is_unit(ty)? {
      Ok("void".to_owned())
    } else {
      self.ty(ty, input)
    }
  }

  fn signatures(&mut self) {
    let unary = self.unary();
    let notifications = self.notifications();
    if unary.is_empty() && notifications.is_empty() {
      return;
    }
    let entries: Vec<String> = self
      .manifest()
      .type_table()
      .iter()
      .map(|(name, definition)| format!("  {}: {},\n", quoted(name), json(&definition.to_json())))
      .collect();
    if entries.is_empty() {
      self.out.push_str("\nconst TYPES: Types = {};\n");
    } else {
      emit!(self.out, "\nconst TYPES: Types = {{\n{}}};", entries.concat());
    }
    if !unary.is_empty() {
      self.out.push_str("\nconst CALLS = {\n");
      for (service, method, call) in unary {
        let returns = call.returns().expect("a unary call has a result type");
        self.signature_start(service, method);
        emit!(self.out, "    returns: {},", json(&returns.to_json()));
        if let Some(error) = call.error_type() {
          emit!(self.out, "    error: {},", json(&error.to_json()));
        }
        if call.is_idempotent() {
          self.out.push_str("    idempotent: true,\n");
        }
        self.out.push_str("    types: TYPES,\n  },\n");
      }
      self.out.push_str("} satisfies Record<string, CallSignature>;\n");
    }
    if !notifications.is_empty() {
      self.out.push_str("\nconst NOTIFICATIONS = {\n");
      for (service, method) in notifications {
        self.signature_start(service, method);
        self.out.push_str("    types: TYPES,\n  },\n");
      }
      self.out.push_str("} satisfies Record<string, NotifySignature>;\n");
    }
  }

  fn signature_start(&mut self, service: &Service, method: &Method) {
    let key = quoted(&method_path(service, method));
    let params: Vec<Value> = method.params().iter().map(|param| param.ty().to_json()).collect();
    emit!(
      self.out,
      "  {key}: {{\n    method: {key},\n    params: {},",
      json(&Value::Array(params))
    );
  }

  fn error_type_class(&mut self, service: &Service, method: &Method, error: &Type) -> Result<(), GenerateError> {
    let class = error_class(service, method);
    let detail = self.ty(error, false)?;
    let path = method_path(service, method);
    emit!(
      self.out,
      "/** {path} failed, with the detail of the server's ERROR typed as the method's error type. */\n\
       export class {class} extends StatusError {{\n  override readonly name = {};\n  declare readonly detail: {detail} \
       | undefined;\n\n  static from(error: StatusError): {class} {{\n    return new {class}(error.code, error.message, \
       {{\n      detail: error.detail,\n      nx: error.nx,\n      meta: error.meta,\n      cause: error.cause,\n    \
       }});\n  }}\n}}",
      quoted(&class)
    );
    Ok(())
  }

  fn params(&self, params: &[Param], input: bool) -> Result<Vec<String>, GenerateError> {
    params
      .iter()
      .map(|param| Ok(format!("{}: {}", param.name(), self.ty(param.ty(), input)?)))
      .collect()
  }

  fn client(&mut self, service: &Service) -> Result<(), GenerateError> {
    self.doc("", service.documentation());
    let class = client_class(service);
    emit!(
      self.out,
      "export class {class} {{\n  readonly #connection: Connection;\n\n  constructor(connection: Connection) \
       {{\n    this.#connection = connection;\n  }}"
    );
    for method in service.server_methods() {
      self.out.push('\n');
      self.doc("  ", method.documentation());
      self.proxy(service, method)?;
    }
    self.out.push_str("}\n");
    Ok(())
  }

  fn proxy(&mut self, service: &Service, method: &Method) -> Result<(), GenerateError> {
    let path = method_path(service, method);
    let key = quoted(&path);
    let mut params = self.params(method.params(), true)?;
    let args = method.params().iter().map(Param::name).collect::<Vec<_>>().join(", ");
    let Some(call) = method.as_call() else {
      params.push("options?: NotifyOptions".to_owned());
      emit!(
        self.out,
        "  {}({}): void {{\n    this.#connection.notify(NOTIFICATIONS[{key}], [{args}], options);\n  }}",
        method.name(),
        params.join(", ")
      );
      return Ok(());
    };
    if let Some(accepts) = call.accepts() {
      params.push(format!("items: AsyncIterable<{}>", self.ty(accepts, true)?));
    }
    params.push("options?: CallOptions".to_owned());
    let params = params.join(", ");
    let name = method.name();
    let stub = quoted(&format!("{path}: streaming is not implemented until M5"));
    let mut used = vec!["this.#connection"];
    used.extend(method.params().iter().map(Param::name));
    if call.accepts().is_some() {
      used.push("items");
    }
    used.push("options");
    let used = used.join(", ");
    let invocation = Invocation {
      params: &params,
      key: &key,
      args: &args,
    };
    match (call.shape(), call.returns()) {
      (Shape::Unary, Some(returns)) => self.unary_proxy(service, method, call, returns, &invocation)?,
      (_, Some(returns)) => emit!(
        self.out,
        "  {name}({params}): Promise<{}> {{\n    void [{used}];\n    return Promise.reject(new Error({stub}));\n  }}",
        self.result(returns, false)?
      ),
      (_, None) => {
        let yields = self.ty(call.yields().expect("a call without a result type yields"), false)?;
        emit!(
          self.out,
          "  {name}({params}): AsyncIterable<{yields}> {{\n    void [{used}];\n    throw new Error({stub});\n  }}"
        );
      }
    }
    Ok(())
  }

  fn unary_proxy(
    &mut self,
    service: &Service,
    method: &Method,
    call: &Call,
    returns: &Type,
    invocation: &Invocation<'_>,
  ) -> Result<(), GenerateError> {
    let Invocation { params, key, args } = invocation;
    let result = self.result(returns, false)?;
    let unit = is_unit(returns)?;
    let invoke = if unit {
      format!("await this.#connection.call(CALLS[{key}], [{args}], options);")
    } else if call.error_type().is_some() {
      format!("return await this.#connection.call<{result}>(CALLS[{key}], [{args}], options);")
    } else {
      format!("return this.#connection.call<{result}>(CALLS[{key}], [{args}], options);")
    };
    emit!(self.out, "  async {}({params}): Promise<{result}> {{", method.name());
    if call.error_type().is_some() {
      let class = error_class(service, method);
      emit!(
        self.out,
        "    try {{\n      {invoke}\n    }} catch (error) {{\n      if (error instanceof StatusError) throw \
         {class}.from(error);\n      throw error;\n    }}"
      );
    } else {
      emit!(self.out, "    {invoke}");
    }
    self.out.push_str("  }\n");
    Ok(())
  }

  fn handlers(&mut self, service: &Service) -> Result<(), GenerateError> {
    let interface = handlers_interface(service);
    emit!(
      self.out,
      "/** The client methods of service `{}`: what the server may call on this client. */\nexport interface \
       {interface} {{",
      service.name()
    );
    for method in service.client_methods() {
      self.doc("  ", method.documentation());
      let mut params = self.params(method.params(), false)?;
      let returns = match method.as_call() {
        None => "void | Promise<void>".to_owned(),
        Some(call) => {
          if let Some(accepts) = call.accepts() {
            params.push(format!("items: AsyncIterable<{}>", self.ty(accepts, false)?));
          }
          match call.returns() {
            Some(returns) => {
              let returns = self.result(returns, true)?;
              format!("{returns} | Promise<{returns}>")
            }
            None => {
              let yields = call.yields().expect("a call without a result type yields");
              format!("AsyncIterable<{}>", self.ty(yields, true)?)
            }
          }
        }
      };
      emit!(self.out, "  {}({}): {returns};", handler_member(method), params.join(", "));
    }
    self.out.push_str("}\n");
    Ok(())
  }
}

const PRIMITIVES: [(Primitive, &str); 17] = [
  (Primitive::Unit, "null"),
  (Primitive::Bool, "boolean"),
  (Primitive::I8, "number"),
  (Primitive::I16, "number"),
  (Primitive::I32, "number"),
  (Primitive::I64, "bigint"),
  (Primitive::U8, "number"),
  (Primitive::U16, "number"),
  (Primitive::U32, "number"),
  (Primitive::U64, "bigint"),
  (Primitive::F32, "number"),
  (Primitive::F64, "number"),
  (Primitive::String, "string"),
  (Primitive::Bytes, "Uint8Array"),
  (Primitive::Timestamp, "Date"),
  (Primitive::Uuid, "string"),
  (Primitive::Any, "unknown"),
];

/// The TypeScript type of a primitive's values, as the runtime's codec decodes them.
fn primitive_ty(primitive: Primitive) -> Result<&'static str, GenerateError> {
  PRIMITIVES
    .iter()
    .find(|(known, _)| *known == primitive)
    .map(|(_, ty)| *ty)
    .ok_or_else(|| unsupported(&Type::from(primitive).to_json()))
}

#[cfg(test)]
mod tests;
