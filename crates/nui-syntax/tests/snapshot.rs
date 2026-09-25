//! Parser snapshot tests: parse representative sources and compare a
//! canonical dump (token stream, AST, diagnostics) against golden text.
//!
//! The dump avoids wildcard match arms on purpose (project convention): a
//! new AST/token variant must update the dumper and the snapshots together.
//!
//! Run `cargo test -p nui-syntax` after intentional parser changes and
//! update the golden strings in the same commit.

use nui_syntax::ast::*;
use nui_syntax::{Diagnostic, Keyword, NumberUnit, Punct, TokenKind, lex, parse};

/// Renders a component-shaped body (shared by components and nodes).
fn dump(source: &str) -> String {
    let outcome = parse(source);
    let mut output = String::new();
    output.push_str("tokens:\n");
    for token in &lex(source).tokens {
        output.push_str("  ");
        output.push_str(&dump_token_kind(&token.kind));
        output.push('\n');
    }
    output.push_str("ast:\n");
    for component in &outcome.document.components {
        dump_component(component, &mut output, 1);
    }
    output.push_str("diagnostics:\n");
    if outcome.diagnostics.is_empty() {
        output.push_str("  (none)\n");
    }
    for diagnostic in &outcome.diagnostics {
        output.push_str("  ");
        dump_diagnostic(diagnostic, &mut output);
        output.push('\n');
    }
    return output;
}

fn indent(level: usize) -> String {
    return "  ".repeat(level);
}

fn dump_component(component: &ComponentDecl, output: &mut String, level: usize) {
    output.push_str(&format!(
        "{}(component {}\n",
        indent(level),
        component.name.name
    ));
    for member in &component.members {
        dump_component_member(member, output, level + 1);
    }
    output.push_str(&format!("{}}}\n", indent(level)));
}

fn dump_component_member(member: &ComponentMember, output: &mut String, level: usize) {
    match member {
        ComponentMember::Property(decl) => {
            output.push_str(&format!("{}(property {}", indent(level), decl.name.name));
            if let Some(declared) = &decl.declared_type {
                output.push_str(&format!(" :{}", declared.name));
            }
            if let Some(init) = &decl.default {
                output.push_str(&format!(" {} ", dump_init_op(init.op)));
                dump_expr(&init.value, output, 0);
            }
            output.push_str(")\n");
        }
        ComponentMember::Signal(decl) => {
            output.push_str(&format!("{}(signal {})\n", indent(level), decl.name.name));
        }
        ComponentMember::Machine(decl) => {
            output.push_str(&format!("{}(machine {}\n", indent(level), decl.name.name));
            for state in &decl.states {
                output.push_str(&format!("{}  (state {}", indent(level), state.name.name));
                if state.enter.is_some() || state.exit.is_some() {
                    output.push('\n');
                    if let Some(enter) = &state.enter {
                        output.push_str(&format!("{}    (enter\n", indent(level)));
                        dump_statements(enter, output, level + 3);
                        output.push_str(&format!("{}    )\n", indent(level)));
                    }
                    if let Some(exit) = &state.exit {
                        output.push_str(&format!("{}    (exit\n", indent(level)));
                        dump_statements(exit, output, level + 3);
                        output.push_str(&format!("{}    )\n", indent(level)));
                    }
                    output.push_str(&format!("{}  )\n", indent(level)));
                } else {
                    output.push_str(")\n");
                }
            }
            for transition in &decl.transitions {
                output.push_str(&format!(
                    "{}  (on {} from {}",
                    indent(level),
                    transition.event.name,
                    transition
                        .from_states
                        .iter()
                        .map(|state| return state.name.as_str())
                        .collect::<Vec<_>>()
                        .join(",")
                ));
                if let Some(guard) = &transition.guard {
                    output.push_str(" when ");
                    dump_expr(guard, output, 0);
                }
                output.push_str(&format!(" => {})\n", transition.to_state.name));
            }
            output.push_str(&format!("{}}}\n", indent(level)));
        }
        ComponentMember::Node(node) => dump_node(node, output, level),
    }
}

fn dump_node(node: &NodeDecl, output: &mut String, level: usize) {
    output.push_str(&format!("{}(node {}\n", indent(level), node.ty.name));
    if let Some(binding) = &node.for_binding {
        output.push_str(&format!(
            "{}  (for {} ",
            indent(level),
            binding.variable.name
        ));
        dump_expr(&binding.iterable, output, level);
        output.push_str(")\n");
    }
    for arg in &node.args {
        match arg {
            NodeArg::Id(id) => {
                output.push_str(&format!("{}  (id {})\n", indent(level), id.name));
            }
            NodeArg::Property(assignment) => dump_assignment(assignment, output, level + 1),
        }
    }
    for member in &node.body {
        match member {
            NodeMember::Assignment(assignment) => {
                dump_assignment(assignment, output, level + 1);
            }
            NodeMember::Handler(handler) => {
                output.push_str(&format!(
                    "{}  (handler {}\n",
                    indent(level),
                    handler.signal.name
                ));
                dump_statements(&handler.effect, output, level + 2);
                output.push_str(&format!("{}  )\n", indent(level)));
            }
            NodeMember::When(when) => {
                output.push_str(&format!("{}  (when ", indent(level)));
                dump_expr(&when.condition, output, level);
                output.push('\n');
                for assignment in &when.assignments {
                    dump_assignment(assignment, output, level + 2);
                }
                output.push_str(&format!("{}  )\n", indent(level)));
            }
            NodeMember::Node(child) => dump_node(child, output, level + 1),
        }
    }
    output.push_str(&format!("{}}}\n", indent(level)));
}

fn dump_assignment(assignment: &PropertyAssignment, output: &mut String, level: usize) {
    output.push_str(&format!(
        "{}(assign {} {} ",
        indent(level),
        assignment
            .target
            .parts
            .iter()
            .map(|part| return part.name.as_str())
            .collect::<Vec<_>>()
            .join("."),
        dump_init_op(assignment.op)
    ));
    dump_expr(&assignment.value, output, level);
    output.push_str(")\n");
}

fn dump_statements(statements: &[Statement], output: &mut String, level: usize) {
    for statement in statements {
        dump_statement(statement, output, level);
    }
}

fn dump_statement(statement: &Statement, output: &mut String, level: usize) {
    match statement {
        Statement::Let { name, value, .. } => {
            output.push_str(&format!("{}(let {} ", indent(level), name.name));
            dump_expr(value, output, level);
            output.push_str(")\n");
        }
        Statement::If {
            condition,
            then_branch,
            else_branch,
            ..
        } => {
            output.push_str(&format!("{}(if\n", indent(level)));
            dump_expr(condition, output, level + 1);
            output.push_str(&format!("{}  (then\n", indent(level)));
            dump_statements(then_branch, output, level + 2);
            output.push_str(&format!("{}  )\n", indent(level)));
            if let Some(else_branch) = else_branch {
                output.push_str(&format!("{}  (else\n", indent(level)));
                dump_statements(else_branch, output, level + 2);
                output.push_str(&format!("{}  )\n", indent(level)));
            }
            output.push_str(&format!("{})\n", indent(level)));
        }
        Statement::Assign {
            target, op, value, ..
        } => {
            output.push_str(&format!(
                "{}(set {} {} ",
                indent(level),
                target
                    .parts
                    .iter()
                    .map(|part| return part.name.as_str())
                    .collect::<Vec<_>>()
                    .join("."),
                dump_assign_op(*op)
            ));
            dump_expr(value, output, level);
            output.push_str(")\n");
        }
        Statement::Emit { signal, .. } => {
            output.push_str(&format!("{}(emit {})\n", indent(level), signal.name));
        }
        Statement::Call { callee, args, .. } => {
            output.push_str(&format!(
                "{}(call {}\n",
                indent(level),
                callee
                    .parts
                    .iter()
                    .map(|part| return part.name.as_str())
                    .collect::<Vec<_>>()
                    .join(".")
            ));
            for arg in args {
                output.push_str(&format!("{}  (arg", indent(level)));
                if let Some(name) = &arg.name {
                    output.push_str(&format!(" {}", name.name));
                }
                output.push(' ');
                dump_expr(&arg.value, output, level + 1);
                output.push_str(")\n");
            }
            output.push_str(&format!("{})\n", indent(level)));
        }
    }
}

fn dump_expr(expr: &Expr, output: &mut String, _level: usize) {
    match expr {
        Expr::Int { value, .. } => output.push_str(&format!("(int {value})")),
        Expr::Float { value, .. } => output.push_str(&format!("(float {value})")),
        Expr::Bool { value, .. } => output.push_str(&format!("(bool {value})")),
        Expr::Auto { .. } => output.push_str("(auto)"),
        Expr::Color { digits, .. } => output.push_str(&format!("(color #{digits})")),
        Expr::Length { length, .. } => {
            output.push_str(&format!("(length {})", dump_length(length)));
        }
        Expr::Duration { duration, .. } => {
            output.push_str(&format!("(duration {}ms)", duration.as_millis_f64()));
        }
        Expr::String { parts, .. } => {
            output.push_str("(string");
            for part in parts {
                match part {
                    StrPart::Text(text) => output.push_str(&format!(" {text:?}")),
                    StrPart::Interp { expr, .. } => {
                        output.push_str(" (hole ");
                        dump_expr(expr, output, _level);
                        output.push(')');
                    }
                }
            }
            output.push(')');
        }
        Expr::Ident { name, .. } => output.push_str(&format!("(ident {name})")),
        Expr::Member { base, name, .. } => {
            output.push_str("(member ");
            dump_expr(base, output, _level);
            output.push_str(&format!(" . {})", name.name));
        }
        Expr::Unary { op, operand, .. } => {
            let name = match op {
                UnaryOp::Neg => "neg",
                UnaryOp::Not => "not",
            };
            output.push_str(&format!("({name} "));
            dump_expr(operand, output, _level);
            output.push(')');
        }
        Expr::Binary { op, lhs, rhs, .. } => {
            output.push_str(&format!("({} ", dump_binary_op(*op)));
            dump_expr(lhs, output, _level);
            output.push(' ');
            dump_expr(rhs, output, _level);
            output.push(')');
        }
        Expr::Ternary {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            output.push_str("(ternary ");
            dump_expr(condition, output, _level);
            output.push(' ');
            dump_expr(then_expr, output, _level);
            output.push(' ');
            dump_expr(else_expr, output, _level);
            output.push(')');
        }
        Expr::Call { callee, args, .. } => {
            output.push_str("(call ");
            dump_expr(callee, output, _level);
            for arg in args {
                output.push_str(" (arg");
                if let Some(name) = &arg.name {
                    output.push_str(&format!(" {}", name.name));
                }
                output.push(' ');
                dump_expr(&arg.value, output, _level);
                output.push(')');
            }
            output.push(')');
        }
        Expr::Error { .. } => output.push_str("(error)"),
    }
}

fn dump_assignment_op_text(op: AssignOp) -> &'static str {
    return match op {
        AssignOp::Set => "=",
        AssignOp::Add => "+=",
        AssignOp::Sub => "-=",
    };
}

fn dump_init_op(op: InitOp) -> &'static str {
    return match op {
        InitOp::Static => "=",
        InitOp::Bind => "<-",
        InitOp::TwoWay => "<=>",
    };
}

fn dump_assign_op(op: AssignOp) -> &'static str {
    return dump_assignment_op_text(op);
}

fn dump_binary_op(op: BinaryOp) -> &'static str {
    return match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "==",
        BinaryOp::NotEq => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    };
}

fn dump_length(length: &nui_core::Length) -> String {
    return match length {
        nui_core::Length::Dp(value) => format!("{value}dp"),
        nui_core::Length::Percent(value) => format!("{value}%"),
        nui_core::Length::Auto => "auto".to_string(),
    };
}

fn dump_token_kind(kind: &TokenKind) -> String {
    return match kind {
        TokenKind::Int { value, unit } => {
            format!("int {value}{}", dump_number_unit(*unit))
        }
        TokenKind::Float { value, unit } => {
            format!("float {value}{}", dump_number_unit(*unit))
        }
        TokenKind::Color(digits) => format!("color #{digits}"),
        TokenKind::Str(value) => format!("string {value:?}"),
        TokenKind::Ident(name) => format!("ident {name}"),
        TokenKind::Keyword(keyword) => format!("keyword {}", dump_keyword(*keyword)),
        TokenKind::Punct(punct) => format!("punct {}", dump_punct(*punct)),
        TokenKind::Eof => "eof".to_string(),
    };
}

fn dump_number_unit(unit: NumberUnit) -> &'static str {
    return match unit {
        NumberUnit::None => "",
        NumberUnit::Dp => "dp",
        NumberUnit::Percent => "%",
        NumberUnit::Millis => "ms",
    };
}

fn dump_keyword(keyword: Keyword) -> &'static str {
    return keyword.as_str();
}

fn dump_punct(punct: Punct) -> &'static str {
    return punct.as_str();
}

fn dump_diagnostic(diagnostic: &Diagnostic, output: &mut String) {
    let severity = match diagnostic.severity {
        nui_syntax::Severity::Error => "error",
        nui_syntax::Severity::Warning => "warning",
    };
    output.push_str(&format!(
        "{} @{}..{}: {}",
        severity, diagnostic.span.start, diagnostic.span.end, diagnostic.message
    ));
    for note in &diagnostic.notes {
        output.push_str(&format!(" | note: {note}"));
    }
}

const COUNTER: &str = r#"
component Counter {
    property count: Int = 0
    signal resetRequested

    Window(id = root, width = 420dp, height = 300dp) {
        title <- "count: {count}"

        Column(spacing = 8dp) {
            Text(content <- "n: {count}", font.size = 20dp)

            Button(label = "+1", enabled <- count < 10) {
                on click => count += 1
            }
        }

        when count >= 10 {
            opacity = 0.6
        }
    }
}
"#;

#[test]
fn snapshot_counter_component() {
    let golden = r#"tokens:
  keyword component
  ident Counter
  punct {
  keyword property
  ident count
  punct :
  ident Int
  punct =
  int 0
  keyword signal
  ident resetRequested
  ident Window
  punct (
  ident id
  punct =
  ident root
  punct ,
  ident width
  punct =
  int 420dp
  punct ,
  ident height
  punct =
  int 300dp
  punct )
  punct {
  ident title
  punct <-
  string "count: {count}"
  ident Column
  punct (
  ident spacing
  punct =
  int 8dp
  punct )
  punct {
  ident Text
  punct (
  ident content
  punct <-
  string "n: {count}"
  punct ,
  ident font
  punct .
  ident size
  punct =
  int 20dp
  punct )
  ident Button
  punct (
  ident label
  punct =
  string "+1"
  punct ,
  ident enabled
  punct <-
  ident count
  punct <
  int 10
  punct )
  punct {
  keyword on
  ident click
  punct =>
  ident count
  punct +=
  int 1
  punct }
  punct }
  keyword when
  ident count
  punct >=
  int 10
  punct {
  ident opacity
  punct =
  float 0.6
  punct }
  punct }
  punct }
  eof
ast:
  (component Counter
    (property count :Int = (int 0))
    (signal resetRequested)
    (node Window
      (id root)
      (assign width = (length 420dp))
      (assign height = (length 300dp))
      (assign title <- (string "count: " (hole (ident count))))
      (node Column
        (assign spacing = (length 8dp))
        (node Text
          (assign content <- (string "n: " (hole (ident count))))
          (assign font.size = (length 20dp))
        }
        (node Button
          (assign label = (string "+1"))
          (assign enabled <- (< (ident count) (int 10)))
          (handler click
            (set count += (int 1))
          )
        }
      }
      (when (>= (ident count) (int 10))
        (assign opacity = (float 0.6))
      )
    }
  }
diagnostics:
  (none)
"#;
    assert_eq!(dump(COUNTER), golden);
}

const PLAYER: &str = r#"
component Player {
    property canStop: Bool = true
    signal play
    signal pause

    machine playback {
        state stopped
        state playing {
            enter => timer.start()
            exit => timer.stop()
        }
        on pause from playing when canStop => stopped
        on play from stopped, paused => playing
    }

    Timer(id = timer) {}

    Text(content <- playback.playing ? "playing" : "stopped")
}
"#;

#[test]
fn snapshot_machine_component() {
    let golden = r#"tokens:
  keyword component
  ident Player
  punct {
  keyword property
  ident canStop
  punct :
  ident Bool
  punct =
  keyword true
  keyword signal
  ident play
  keyword signal
  ident pause
  keyword machine
  ident playback
  punct {
  keyword state
  ident stopped
  keyword state
  ident playing
  punct {
  keyword enter
  punct =>
  ident timer
  punct .
  ident start
  punct (
  punct )
  keyword exit
  punct =>
  ident timer
  punct .
  ident stop
  punct (
  punct )
  punct }
  keyword on
  ident pause
  keyword from
  ident playing
  keyword when
  ident canStop
  punct =>
  ident stopped
  keyword on
  ident play
  keyword from
  ident stopped
  punct ,
  ident paused
  punct =>
  ident playing
  punct }
  ident Timer
  punct (
  ident id
  punct =
  ident timer
  punct )
  punct {
  punct }
  ident Text
  punct (
  ident content
  punct <-
  ident playback
  punct .
  ident playing
  punct ?
  string "playing"
  punct :
  string "stopped"
  punct )
  punct }
  eof
ast:
  (component Player
    (property canStop :Bool = (bool true))
    (signal play)
    (signal pause)
    (machine playback
      (state stopped)
      (state playing
        (enter
          (call timer.start
          )
        )
        (exit
          (call timer.stop
          )
        )
      )
      (on pause from playing when (ident canStop) => stopped)
      (on play from stopped,paused => playing)
    }
    (node Timer
      (id timer)
    }
    (node Text
      (assign content <- (ternary (member (ident playback) . playing) (string "playing") (string "stopped")))
    }
  }
diagnostics:
  (none)
"#;
    assert_eq!(dump(PLAYER), golden);
}

const FOR_LIST: &str = r#"
component List {
    For(item in root.items) {
        Row(key = item.id) {
            Text(content <- item.label)
        }
    }
}
"#;

#[test]
fn snapshot_for_component() {
    let golden = r#"tokens:
  keyword component
  ident List
  punct {
  ident For
  punct (
  ident item
  keyword in
  ident root
  punct .
  ident items
  punct )
  punct {
  ident Row
  punct (
  ident key
  punct =
  ident item
  punct .
  ident id
  punct )
  punct {
  ident Text
  punct (
  ident content
  punct <-
  ident item
  punct .
  ident label
  punct )
  punct }
  punct }
  punct }
  eof
ast:
  (component List
    (node For
      (for item (member (ident root) . items))
      (node Row
        (assign key = (member (ident item) . id))
        (node Text
          (assign content <- (member (ident item) . label))
        }
      }
    }
  }
diagnostics:
  (none)
"#;
    assert_eq!(dump(FOR_LIST), golden);
}

const BROKEN: &str = r#"
component A { property x: Int = component B { property y = 1 }
"#;

#[test]
fn snapshot_error_recovery() {
    let golden = r#"tokens:
  keyword component
  ident A
  punct {
  keyword property
  ident x
  punct :
  ident Int
  punct =
  keyword component
  ident B
  punct {
  keyword property
  ident y
  punct =
  int 1
  punct }
  eof
ast:
  (component A
    (property x :Int = (error))
    (node B
      (assign y = (int 1))
    }
  }
diagnostics:
  error @33..42: expected an expression, found keyword `component`
  error @47..55: expected a property assignment, handler, `when` block, or child node, found keyword `property`
  error @64..64: expected `}` before end of file
"#;
    assert_eq!(dump(BROKEN), golden);
}

#[test]
fn snapshots_are_stable_under_whitespace() {
    // Formatting-only differences must not change the dump's AST section.
    let compact = "component A { property n: Int = 1 + 2 Text(x <- n * 2) {} }";
    let spread = r#"
        component   A   {
            property n : Int = 1 + 2
            Text( x <- n * 2 ) { }
        }
    "#;
    assert_eq!(
        dump_ast_section(compact),
        dump_ast_section(spread),
        "AST must be whitespace-insensitive"
    );
}

/// Extracts the `ast:` section of a dump for whitespace-insensitivity checks.
fn dump_ast_section(source: &str) -> String {
    let full = dump(source);
    let start = full
        .find("ast:\n")
        .expect("dump always contains an ast section");
    let end = full
        .find("diagnostics:\n")
        .expect("dump always contains a diagnostics section");
    return full[start..end].to_string();
}
