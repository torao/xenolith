use super::*;

fn tokens(input: &str) -> Vec<Token> {
  tokenize(input).expect("tokenizes").into_iter().map(|spanned| spanned.token).collect()
}

/// The reason `input` fails to tokenize with.
fn error(input: &str) -> String {
  tokenize(input).expect_err("fails").0
}

#[test]
fn star_is_a_wildcard_at_the_start_of_a_step_and_multiplication_after_an_operand() {
  assert_eq!(tokens("*"), [Token::Star]);
  assert_eq!(tokens("@*"), [Token::At, Token::Star]);
  assert_eq!(tokens("a/*"), [Token::Name { prefix: None, local: "a".into() }, Token::Slash, Token::Star]);
  assert_eq!(
    tokens("2 * 3"),
    [Token::Number(2.0), Token::Multiply, Token::Number(3.0)],
    "after a number, * is multiplication"
  );
}

#[test]
fn operator_names_are_names_where_a_name_belongs() {
  assert_eq!(tokens("div"), [Token::Name { prefix: None, local: "div".into() }]);
  assert_eq!(tokens("1 div 2"), [Token::Number(1.0), Token::Div, Token::Number(2.0)]);
  assert_eq!(
    tokens("child::div"),
    [Token::Axis(Axis::Child), Token::ColonColon, Token::Name { prefix: None, local: "div".into() }]
  );
}

#[test]
fn a_name_before_a_parenthesis_is_a_node_type_or_a_function() {
  assert_eq!(tokens("text()"), [Token::NodeType(NodeTypeName::Text), Token::LeftParen, Token::RightParen]);
  assert_eq!(
    tokens("count(a)"),
    [
      Token::Function { prefix: None, local: "count".into() },
      Token::LeftParen,
      Token::Name { prefix: None, local: "a".into() },
      Token::RightParen
    ]
  );
  assert_eq!(tokens("text"), [Token::Name { prefix: None, local: "text".into() }], "without ( it is a name");
}

#[test]
fn a_name_before_a_double_colon_is_an_axis() {
  assert_eq!(tokens("ancestor-or-self::"), [Token::Axis(Axis::AncestorOrSelf), Token::ColonColon]);
  assert!(tokenize("nosuch::a").is_err(), "an unknown axis is refused where an axis must be");
}

#[test]
fn reads_qualified_names_and_wildcards() {
  assert_eq!(tokens("p:a"), [Token::Name { prefix: Some("p".into()), local: "a".into() }]);
  assert_eq!(tokens("p:*"), [Token::NamespaceWildcard("p".into())]);
  assert_eq!(tokens("$p:v"), [Token::Variable { prefix: Some("p".into()), local: "v".into() }]);
}

#[test]
fn reads_numbers_literals_and_the_dot_tokens() {
  assert_eq!(tokens("1 1.5 .5"), [Token::Number(1.0), Token::Number(1.5), Token::Number(0.5)]);
  assert_eq!(tokens("'a' \"b\""), [Token::Literal("a".into()), Token::Literal("b".into())]);
  assert_eq!(tokens(". .. ./."), [Token::Dot, Token::DotDot, Token::Dot, Token::Slash, Token::Dot]);
}

#[test]
fn reports_what_it_could_not_read_and_where() {
  assert!(error("'unclosed").contains("never closed"));
  assert!(error("a # b").contains("unexpected character"));
  assert!(error("$").contains("expected a name"));
  assert_eq!(tokenize("a # b").expect_err("fails").1, 2, "the byte offset of the character");
}
