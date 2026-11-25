//! The TypeScript back end (§12.4): one module for the `@pavia/client` runtime, holding the contract
//! fingerprint, a type per named type, a client class per service, a typed error class per method with
//! an error type, and a handler interface per service with client methods.
//!
//! Names are the manifest's as they are; class names are derived from them (`chat` gives `ChatClient`,
//! `chat/send` gives `ChatSendError`). A name the TypeScript code cannot carry, a keyword, a global the
//! generated code uses, or a member every object has, fails generation instead of being renamed.

use std::fmt::Write as _;

use pavia_contract::manifest::{Call, Manifest, Method, Param, Service, Shape};
use pavia_contract::schema::{Enum, Field, Primitive, Struct, Type};
use serde_json::Value;

use crate::GenerateError;
use crate::front::{Body, Form, Front, Names, body, form, pascal, unsupported};

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

const MEMBERS: &[&str] = &[
  "__proto__",
  "constructor",
  "hasOwnProperty",
  "isPrototypeOf",
  "propertyIsEnumerable",
  "then",
  "toLocaleString",
  "toString",
  "valueOf",
];

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
  check_types(manifest)?;
  let names = claim_names(&front)?;
  check_members(manifest, &names)?;
  Ok(Writer::new(&front).module())
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
  for name in RUNTIME.iter().chain(CONSTANTS) {
    names.claim(name, "the generated code")?;
  }
  for name in front.manifest.type_table().keys() {
    if KEYWORDS.contains(&name.as_str()) || GLOBALS.contains(&name.as_str()) || names.contains(name) {
      return Err(reserved("type", name));
    }
    names.claim(name, format!("type `{name}`"))?;
  }
  for name in front.manifest.type_table().keys().filter(|name| front.has_input_form(name)) {
    names.claim(&format!("{name}Input"), format!("the input form of type `{name}`"))?;
  }
  for service in front.manifest.services() {
    let class = pascal(service.name());
    names.claim(
      &format!("{class}Client"),
      format!("the client of service `{}`", service.name()),
    )?;
    if !service.client_methods().is_empty() {
      names.claim(
        &format!("{class}Handlers"),
        format!("the handlers of service `{}`", service.name()),
      )?;
    }
    for method in service.server_methods().iter().filter(|method| error_class(method)) {
      names.claim(
        &format!("{class}{}Error", pascal(method.name())),
        format!("the error of `{}/{}`", service.name(), method.name()),
      )?;
    }
  }
  Ok(names)
}

/// Checks that every type the module writes is in a form this back end knows, so the writer can
/// count on it.
fn check_types(manifest: &Manifest) -> Result<(), GenerateError> {
  for definition in manifest.type_table().values() {
    match body(definition)? {
      Body::Struct(fields) => known_fields(fields)?,
      Body::Enum(variants) => {
        for ty in variants.variants().iter().filter_map(|variant| variant.ty()) {
          known(ty)?;
        }
      }
      Body::Alias(target) => known(target)?,
    }
  }
  for method in manifest
    .services()
    .iter()
    .flat_map(|service| service.server_methods().iter().chain(service.client_methods()))
  {
    for param in method.params() {
      known(param.ty())?;
    }
    if let Some(call) = method.as_call() {
      for ty in [call.accepts(), call.yields(), call.returns(), call.error_type()]
        .into_iter()
        .flatten()
      {
        known(ty)?;
      }
    }
  }
  Ok(())
}

fn known_fields(fields: &Struct) -> Result<(), GenerateError> {
  fields.fields().iter().try_for_each(|field| known(field.ty()))
}

fn known(ty: &Type) -> Result<(), GenerateError> {
  match form(ty)? {
    Form::Primitive(primitive) => primitive_ty(primitive).map(drop),
    Form::Option(inner) | Form::List(inner) | Form::Map(inner) => known(inner),
    Form::Tuple(items) => items.iter().try_for_each(known),
    Form::Struct(fields) => known_fields(fields),
    Form::Ref(_) => Ok(()),
  }
}

/// `ty` in a form this back end knows, which [`check_types`] made sure of.
fn checked(ty: &Type) -> Form<'_> {
  form(ty).expect("check_types refuses a type in a form this back end does not know")
}

fn check_members(manifest: &Manifest, names: &Names) -> Result<(), GenerateError> {
  for (name, definition) in manifest.type_table() {
    let fields: Vec<&str> = match body(definition).expect("check_types refuses a definition it does not know") {
      Body::Struct(fields) => fields.fields().iter().map(Field::name).collect(),
      Body::Enum(variants) => variants.variants().iter().map(|variant| variant.name()).collect(),
      Body::Alias(_) => Vec::new(),
    };
    if let Some(field) = fields.into_iter().find(|field| *field == "__proto__") {
      return Err(reserved(format!("field or variant of type `{name}`"), field));
    }
  }
  for service in manifest.services() {
    for (side, methods) in [("server", service.server_methods()), ("client", service.client_methods())] {
      for method in methods {
        let what = format!("{side} method of service `{}`", service.name());
        if MEMBERS.contains(&method.name()) {
          return Err(reserved(what, method.name()));
        }
        for param in method.params() {
          let name = param.name();
          if KEYWORDS.contains(&name) || GLOBALS.contains(&name) || LOCALS.contains(&name) || names.contains(name) {
            return Err(reserved(format!("param of {what} `{}`", method.name()), name));
          }
        }
      }
    }
  }
  Ok(())
}

fn error_class(method: &Method) -> bool {
  method
    .as_call()
    .is_some_and(|call| call.shape() == Shape::Unary && call.error_type().is_some())
}

fn json(value: &Value) -> String {
  serde_json::to_string(value).expect("a JSON value serializes")
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

  fn module(mut self) -> String {
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
    writeln!(
      self.out,
      "export const CONTRACT: Uint8Array = Uint8Array.from([\n  {},\n]);",
      rows.join(",\n  ")
    )
    .expect("writing to a String");
    for (name, definition) in self.manifest().type_table() {
      self.out.push('\n');
      let definition = body(definition).expect("check_types refuses a definition it does not know");
      self.definition(name, &definition, false);
      if self.front.has_input_form(name) {
        self.out.push('\n');
        self.definition(name, &definition, true);
      }
    }
    self.signatures();
    for (service, method, call) in self.unary() {
      if let Some(error) = call.error_type() {
        self.out.push('\n');
        self.error_class(service, method, error);
      }
    }
    for service in self.manifest().services() {
      self.out.push('\n');
      self.client(service);
      if !service.client_methods().is_empty() {
        self.out.push('\n');
        self.handlers(service);
      }
    }
    self.out
  }

  fn imports(&mut self) {
    let services = self.manifest().services();
    let calls = services
      .iter()
      .flat_map(Service::server_methods)
      .any(|method| method.as_call().is_some());
    let unary = !self.unary().is_empty();
    let notifications = !self.notifications().is_empty();
    let errors = self.unary().iter().any(|(_, _, call)| call.error_type().is_some());
    let used = [
      ("type CallOptions", calls),
      ("type CallSignature", unary),
      ("type Connection", !services.is_empty()),
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
      writeln!(self.out, "{line}").expect("writing to a String");
    } else {
      writeln!(self.out, "import {{\n  {},\n}} from \"@pavia/client\";", names.join(",\n  ")).expect("writing to a String");
    }
  }

  fn doc(&mut self, indent: &str, text: Option<&str>) {
    let Some(text) = text else { return };
    writeln!(self.out, "{indent}/**").expect("writing to a String");
    for line in comment_lines(text) {
      if line.is_empty() {
        writeln!(self.out, "{indent} *").expect("writing to a String");
      } else {
        writeln!(self.out, "{indent} * {line}").expect("writing to a String");
      }
    }
    writeln!(self.out, "{indent} */").expect("writing to a String");
  }

  fn definition(&mut self, name: &str, definition: &Body<'_>, input: bool) {
    let declared = if input { format!("{name}Input") } else { name.to_owned() };
    match definition {
      Body::Struct(fields) if fields.fields().is_empty() => {
        writeln!(self.out, "export interface {declared} {{}}").expect("writing to a String");
      }
      Body::Struct(fields) => {
        writeln!(self.out, "export interface {declared} {{").expect("writing to a String");
        for field in fields.fields() {
          let field = self.field(field, input);
          writeln!(self.out, "  {field};").expect("writing to a String");
        }
        self.out.push_str("}\n");
      }
      Body::Enum(variants) => {
        let union = self.variants(variants, input);
        writeln!(self.out, "export type {declared} = {union};").expect("writing to a String");
      }
      Body::Alias(target) => {
        let ty = self.ty(target, input);
        writeln!(self.out, "export type {declared} = {ty};").expect("writing to a String");
      }
    }
  }

  fn field(&self, field: &Field, input: bool) -> String {
    let mark = if field.is_optional() { "?" } else { "" };
    format!("{}{mark}: {}", field.name(), self.ty(field.ty(), input))
  }

  fn variants(&self, variants: &Enum, input: bool) -> String {
    let mut members: Vec<String> = variants
      .variants()
      .iter()
      .map(|variant| match variant.ty() {
        None => format!("{{ kind: {} }}", quoted(variant.name())),
        Some(ty) => format!("{{ kind: {}; value: {} }}", quoted(variant.name()), self.ty(ty, input)),
      })
      .collect();
    if variants.is_open() && !input {
      members.push("{ kind: \"Unknown\"; name: string }".to_owned());
    }
    if members.is_empty() {
      "never".to_owned()
    } else {
      members.join(" | ")
    }
  }

  /// The TypeScript type of the values of `ty`: as decoded, or, with `input`, as a sender writes them.
  fn ty(&self, ty: &Type, input: bool) -> String {
    match checked(ty) {
      Form::Primitive(primitive) => primitive_ty(primitive)
        .expect("check_types refuses a primitive this back end does not map")
        .to_owned(),
      Form::Option(inner) => format!("{} | null", self.ty(inner, input)),
      Form::List(inner) if matches!(checked(inner), Form::Option(_)) => format!("({})[]", self.ty(inner, input)),
      Form::List(inner) => format!("{}[]", self.ty(inner, input)),
      Form::Map(inner) => format!("Map<string, {}>", self.ty(inner, input)),
      Form::Tuple(items) => {
        let items: Vec<String> = items.iter().map(|item| self.ty(item, input)).collect();
        format!("[{}]", items.join(", "))
      }
      Form::Struct(fields) => self.inline_struct(fields, input),
      Form::Ref(name) if input && self.front.has_input_form(name) => format!("{name}Input"),
      Form::Ref(name) => name.to_owned(),
    }
  }

  fn inline_struct(&self, fields: &Struct, input: bool) -> String {
    if fields.fields().is_empty() {
      return "{}".to_owned();
    }
    let fields: Vec<String> = fields.fields().iter().map(|field| self.field(field, input)).collect();
    format!("{{ {} }}", fields.join("; "))
  }

  /// The type of a value a callee answers with: `void` for `unit`, and the form `input` asks for.
  fn result(&self, ty: &Type, input: bool) -> String {
    match checked(ty) {
      Form::Primitive(Primitive::Unit) => "void".to_owned(),
      _ => self.ty(ty, input),
    }
  }

  fn signatures(&mut self) {
    let unary = self.unary();
    let notifications = self.notifications();
    if unary.is_empty() && notifications.is_empty() {
      return;
    }
    let table: serde_json::Map<String, Value> = self
      .manifest()
      .type_table()
      .iter()
      .map(|(name, definition)| (name.clone(), definition.to_json()))
      .collect();
    let table = serde_json::to_string_pretty(&Value::Object(table)).expect("a JSON value serializes");
    write!(self.out, "\nconst TYPES: Types = {table};\n").expect("writing to a String");
    if !unary.is_empty() {
      self.out.push_str("\nconst CALLS = {\n");
      for (service, method, call) in unary {
        let returns = call.returns().expect("a unary call has a result type");
        self.signature_start(service, method);
        writeln!(self.out, "    returns: {},", json(&returns.to_json())).expect("writing to a String");
        if let Some(error) = call.error_type() {
          writeln!(self.out, "    error: {},", json(&error.to_json())).expect("writing to a String");
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
    let key = quoted(&format!("{}/{}", service.name(), method.name()));
    let params: Vec<Value> = method.params().iter().map(|param| param.ty().to_json()).collect();
    writeln!(
      self.out,
      "  {key}: {{\n    method: {key},\n    params: {},",
      json(&Value::Array(params))
    )
    .expect("writing to a String");
  }

  fn error_class(&mut self, service: &Service, method: &Method, error: &Type) {
    let class = format!("{}{}Error", pascal(service.name()), pascal(method.name()));
    let detail = self.ty(error, false);
    let path = format!("{}/{}", service.name(), method.name());
    writeln!(
      self.out,
      "/** {path} failed: the server's ERROR, with its detail typed as the method's error type. */\n\
       export class {class} extends StatusError {{\n  override readonly name = {};\n  declare readonly detail: {detail} \
       | undefined;\n\n  static from(error: StatusError): {class} {{\n    return new {class}(error.code, error.message, \
       {{\n      detail: error.detail,\n      nx: error.nx,\n      meta: error.meta,\n      cause: error.cause,\n    \
       }});\n  }}\n}}",
      quoted(&class)
    )
    .expect("writing to a String");
  }

  fn params(&self, params: &[Param], input: bool) -> Vec<String> {
    params
      .iter()
      .map(|param| format!("{}: {}", param.name(), self.ty(param.ty(), input)))
      .collect()
  }

  fn client(&mut self, service: &Service) {
    self.doc("", service.documentation());
    let class = pascal(service.name());
    writeln!(
      self.out,
      "export class {class}Client {{\n  readonly #connection: Connection;\n\n  constructor(connection: Connection) \
       {{\n    this.#connection = connection;\n  }}"
    )
    .expect("writing to a String");
    for method in service.server_methods() {
      self.out.push('\n');
      self.doc("  ", method.documentation());
      self.proxy(service, method);
    }
    self.out.push_str("}\n");
  }

  fn proxy(&mut self, service: &Service, method: &Method) {
    let path = format!("{}/{}", service.name(), method.name());
    let key = quoted(&path);
    let mut params = self.params(method.params(), true);
    let args = method.params().iter().map(Param::name).collect::<Vec<_>>().join(", ");
    let Some(call) = method.as_call() else {
      params.push("options?: NotifyOptions".to_owned());
      writeln!(
        self.out,
        "  {}({}): void {{\n    this.#connection.notify(NOTIFICATIONS[{key}], [{args}], options);\n  }}",
        method.name(),
        params.join(", ")
      )
      .expect("writing to a String");
      return;
    };
    if let Some(accepts) = call.accepts() {
      params.push(format!("items: AsyncIterable<{}>", self.ty(accepts, true)));
    }
    params.push("options?: CallOptions".to_owned());
    let params = params.join(", ");
    let name = method.name();
    let stub = quoted(&format!("{path}: streaming is not implemented until M5"));
    let mut used: Vec<&str> = method.params().iter().map(Param::name).collect();
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
      (Shape::Unary, Some(returns)) => self.unary_proxy(service, method, call, returns, &invocation),
      (_, Some(returns)) => writeln!(
        self.out,
        "  {name}({params}): Promise<{}> {{\n    void [{used}];\n    return Promise.reject(new Error({stub}));\n  }}",
        self.result(returns, false)
      )
      .expect("writing to a String"),
      (_, None) => {
        let yields = self.ty(call.yields().expect("a call without a result type yields"), false);
        writeln!(
          self.out,
          "  {name}({params}): AsyncIterable<{yields}> {{\n    void [{used}];\n    throw new Error({stub});\n  }}"
        )
        .expect("writing to a String");
      }
    }
  }

  fn unary_proxy(&mut self, service: &Service, method: &Method, call: &Call, returns: &Type, invocation: &Invocation<'_>) {
    let Invocation { params, key, args } = invocation;
    let result = self.result(returns, false);
    let unit = result == "void";
    let invoke = if unit {
      format!("await this.#connection.call(CALLS[{key}], [{args}], options);")
    } else if call.error_type().is_some() {
      format!("return await this.#connection.call<{result}>(CALLS[{key}], [{args}], options);")
    } else {
      format!("return this.#connection.call<{result}>(CALLS[{key}], [{args}], options);")
    };
    writeln!(self.out, "  async {}({params}): Promise<{result}> {{", method.name()).expect("writing to a String");
    if call.error_type().is_some() {
      let class = format!("{}{}Error", pascal(service.name()), pascal(method.name()));
      writeln!(
        self.out,
        "    try {{\n      {invoke}\n    }} catch (error) {{\n      if (error instanceof StatusError) throw \
         {class}.from(error);\n      throw error;\n    }}"
      )
      .expect("writing to a String");
    } else {
      writeln!(self.out, "    {invoke}").expect("writing to a String");
    }
    self.out.push_str("  }\n");
  }

  fn handlers(&mut self, service: &Service) {
    let class = pascal(service.name());
    writeln!(
      self.out,
      "/** The client methods of service `{}`: what the server may call on this client. */\nexport interface \
       {class}Handlers {{",
      comment_lines(service.name()).join(" ")
    )
    .expect("writing to a String");
    for method in service.client_methods() {
      self.doc("  ", method.documentation());
      let mut params = self.params(method.params(), false);
      let returns = match method.as_call() {
        None => "void | Promise<void>".to_owned(),
        Some(call) => {
          if let Some(accepts) = call.accepts() {
            params.push(format!("items: AsyncIterable<{}>", self.ty(accepts, false)));
          }
          match call.returns() {
            Some(returns) => {
              let returns = self.result(returns, true);
              format!("{returns} | Promise<{returns}>")
            }
            None => {
              let yields = call.yields().expect("a call without a result type yields");
              format!("AsyncIterable<{}>", self.ty(yields, true))
            }
          }
        }
      };
      writeln!(self.out, "  {}({}): {returns};", method.name(), params.join(", ")).expect("writing to a String");
    }
    self.out.push_str("}\n");
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
