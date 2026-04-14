use std::path::{Path, PathBuf};

use crate::TestItem;

#[derive(Debug, Clone, PartialEq)]
pub enum PathSpec {
    File(PathBuf),
    FileLine(PathBuf, u32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum FilterExpr {
    Substring(String),
    And(Box<FilterExpr>, Box<FilterExpr>),
    Or(Box<FilterExpr>, Box<FilterExpr>),
    Not(Box<FilterExpr>),
}

#[derive(Debug)]
pub struct TestFilter {
    pub path_specs: Vec<PathSpec>,
    pub expr: Option<FilterExpr>,
    pub marker_expr: Option<FilterExpr>,
}

#[derive(Debug)]
pub enum FilterError {
    Parse(String),
    PathSpec(String),
}

impl std::fmt::Display for FilterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(msg) => write!(f, "invalid filter expression: {msg}"),
            Self::PathSpec(msg) => write!(f, "invalid path spec: {msg}"),
        }
    }
}

impl std::error::Error for FilterError {}

// --- PathSpec ---

impl PathSpec {
    /// # Errors
    /// Returns `FilterError::PathSpec` if the path string is empty.
    pub fn parse(s: &str) -> Result<Self, FilterError> {
        if let Some((path, line_str)) = s.rsplit_once(':')
            && let Ok(line) = line_str.parse::<u32>()
        {
            return Ok(Self::FileLine(PathBuf::from(path), line));
        }
        if s.is_empty() {
            return Err(FilterError::PathSpec("empty path".into()));
        }
        Ok(Self::File(PathBuf::from(s)))
    }

    #[must_use]
    pub fn matches(&self, test: &TestItem) -> bool {
        match self {
            Self::File(spec_path) => test
                .file_path
                .as_ref()
                .is_some_and(|fp| path_spec_matches(fp, spec_path)),
            Self::FileLine(spec_path, line) => {
                test.file_path
                    .as_ref()
                    .is_some_and(|fp| path_spec_matches(fp, spec_path))
                    && test.line_number == Some(*line)
            }
        }
    }
}

/// A path spec matches a test file if the spec is a trailing suffix of the
/// test path (so `math.py` matches `tests/math.py`) or an ancestor directory
/// of the test path (so `tests` matches `tests/math.py`).
fn path_spec_matches(haystack: &Path, needle: &Path) -> bool {
    path_suffix_match(haystack, needle) || path_prefix_match(haystack, needle)
}

/// Returns true if `haystack` ends with the components of `needle`.
fn path_suffix_match(haystack: &Path, needle: &Path) -> bool {
    let h: Vec<_> = haystack.components().rev().collect();
    let n: Vec<_> = needle.components().rev().collect();
    if n.len() > h.len() {
        return false;
    }
    n.iter().zip(h.iter()).all(|(a, b)| a == b)
}

/// Returns true if `haystack` starts with the components of `needle`, so
/// a directory spec matches every test file underneath it.
fn path_prefix_match(haystack: &Path, needle: &Path) -> bool {
    let h: Vec<_> = haystack.components().collect();
    let n: Vec<_> = needle.components().collect();
    if n.is_empty() || n.len() > h.len() {
        return false;
    }
    n.iter().zip(h.iter()).all(|(a, b)| a == b)
}

// --- FilterExpr parser ---

#[derive(Debug, Clone, PartialEq)]
enum Token {
    Ident(String),
    LParen,
    RParen,
    And,
    Or,
    Not,
}

fn tokenize(input: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut chars = input.chars().peekable();
    while let Some(&ch) = chars.peek() {
        match ch {
            ' ' | '\t' => {
                chars.next();
            }
            '(' => {
                tokens.push(Token::LParen);
                chars.next();
            }
            ')' => {
                tokens.push(Token::RParen);
                chars.next();
            }
            _ => {
                let mut word = String::new();
                while let Some(&c) = chars.peek() {
                    if c == ' ' || c == '\t' || c == '(' || c == ')' {
                        break;
                    }
                    word.push(c);
                    chars.next();
                }
                match word.as_str() {
                    "and" => tokens.push(Token::And),
                    "or" => tokens.push(Token::Or),
                    "not" => tokens.push(Token::Not),
                    _ => tokens.push(Token::Ident(word)),
                }
            }
        }
    }
    tokens
}

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
}

impl Parser {
    fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn advance(&mut self) -> Option<&Token> {
        let tok = self.tokens.get(self.pos);
        self.pos += 1;
        tok
    }

    fn parse_expr(&mut self) -> Result<FilterExpr, FilterError> {
        self.parse_or()
    }

    fn parse_or(&mut self) -> Result<FilterExpr, FilterError> {
        let mut left = self.parse_and()?;
        while self.peek() == Some(&Token::Or) {
            self.advance();
            let right = self.parse_and()?;
            left = FilterExpr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_and(&mut self) -> Result<FilterExpr, FilterError> {
        let mut left = self.parse_not()?;
        while self.peek() == Some(&Token::And) {
            self.advance();
            let right = self.parse_not()?;
            left = FilterExpr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_not(&mut self) -> Result<FilterExpr, FilterError> {
        if self.peek() == Some(&Token::Not) {
            self.advance();
            let inner = self.parse_not()?;
            return Ok(FilterExpr::Not(Box::new(inner)));
        }
        self.parse_atom()
    }

    fn parse_atom(&mut self) -> Result<FilterExpr, FilterError> {
        match self.advance() {
            Some(Token::LParen) => {
                let expr = self.parse_expr()?;
                match self.advance() {
                    Some(Token::RParen) => Ok(expr),
                    _ => Err(FilterError::Parse("expected closing ')'".into())),
                }
            }
            Some(Token::Ident(s)) => Ok(FilterExpr::Substring(s.clone())),
            Some(tok) => Err(FilterError::Parse(format!("unexpected token: {tok:?}"))),
            None => Err(FilterError::Parse("unexpected end of expression".into())),
        }
    }
}

impl FilterExpr {
    /// Match against a set of tag strings instead of test identity fields.
    #[must_use]
    pub fn matches_tags(&self, tags: &[String]) -> bool {
        match self {
            Self::Substring(s) => {
                let needle = s.to_lowercase();
                tags.iter().any(|t| t.to_lowercase().contains(&needle))
            }
            Self::And(a, b) => a.matches_tags(tags) && b.matches_tags(tags),
            Self::Or(a, b) => a.matches_tags(tags) || b.matches_tags(tags),
            Self::Not(inner) => !inner.matches_tags(tags),
        }
    }

    /// # Errors
    /// Returns `FilterError::Parse` if the expression is malformed.
    pub fn parse(input: &str) -> Result<Self, FilterError> {
        let tokens = tokenize(input);
        if tokens.is_empty() {
            return Err(FilterError::Parse("empty expression".into()));
        }
        let mut parser = Parser::new(tokens);
        let expr = parser.parse_expr()?;
        if parser.pos < parser.tokens.len() {
            return Err(FilterError::Parse(format!(
                "unexpected token at position {}",
                parser.pos
            )));
        }
        Ok(expr)
    }

    #[must_use]
    pub fn matches(&self, test: &TestItem) -> bool {
        let id = test.id().to_lowercase();
        let name = test.name.to_lowercase();
        let module = test.module_path.to_lowercase();
        let display = test.display_name.as_deref().unwrap_or("").to_lowercase();
        let qualified = if test.groups.is_empty() {
            String::new()
        } else {
            let mut parts = test.groups.clone();
            parts.push(
                test.display_name
                    .as_deref()
                    .unwrap_or(&test.name)
                    .to_owned(),
            );
            parts.join(" > ").to_lowercase()
        };
        self.matches_inner(&id, &name, &module, &display, &qualified)
    }

    fn matches_inner(
        &self,
        id: &str,
        name: &str,
        module: &str,
        display: &str,
        qualified: &str,
    ) -> bool {
        match self {
            Self::Substring(s) => {
                let needle = s.to_lowercase();
                id.contains(&needle)
                    || name.contains(&needle)
                    || module.contains(&needle)
                    || display.contains(&needle)
                    || qualified.contains(&needle)
            }
            Self::And(a, b) => {
                a.matches_inner(id, name, module, display, qualified)
                    && b.matches_inner(id, name, module, display, qualified)
            }
            Self::Or(a, b) => {
                a.matches_inner(id, name, module, display, qualified)
                    || b.matches_inner(id, name, module, display, qualified)
            }
            Self::Not(inner) => !inner.matches_inner(id, name, module, display, qualified),
        }
    }
}

// --- TestFilter ---

impl TestFilter {
    /// # Errors
    /// Returns `FilterError` if any path spec or filter expression is invalid.
    pub fn from_args(
        paths: &[String],
        filter: Option<&str>,
        markers: Option<&str>,
    ) -> Result<Self, FilterError> {
        let path_specs = paths
            .iter()
            .map(|s| PathSpec::parse(s))
            .collect::<Result<Vec<_>, _>>()?;
        let expr = filter.map(FilterExpr::parse).transpose()?;
        let marker_expr = markers.map(FilterExpr::parse).transpose()?;
        Ok(Self {
            path_specs,
            expr,
            marker_expr,
        })
    }

    #[must_use]
    pub fn matches(&self, test: &TestItem) -> bool {
        let path_ok =
            self.path_specs.is_empty() || self.path_specs.iter().any(|spec| spec.matches(test));
        let expr_ok = self.expr.as_ref().is_none_or(|expr| expr.matches(test));
        let marker_ok = self
            .marker_expr
            .as_ref()
            .is_none_or(|expr| expr.matches_tags(&test.tags));
        path_ok && expr_ok && marker_ok
    }

    #[must_use]
    pub fn apply(&self, tests: Vec<TestItem>) -> Vec<TestItem> {
        if self.is_empty() {
            return tests;
        }
        tests.into_iter().filter(|t| self.matches(t)).collect()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.path_specs.is_empty() && self.expr.is_none() && self.marker_expr.is_none()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test(name: &str, file: &str, line: u32) -> TestItem {
        TestItem {
            name: name.into(),
            module_path: file.replace('/', ".").replace(".py", ""),
            file_path: Some(PathBuf::from(file)),
            line_number: Some(line),
            display_name: None,
            expected_assertions: vec![],
            ..Default::default()
        }
    }

    // --- FilterExpr::parse tests ---

    #[test]
    fn parse_simple_substring() {
        let expr = FilterExpr::parse("test_add").unwrap();
        assert_eq!(expr, FilterExpr::Substring("test_add".into()));
    }

    #[test]
    fn parse_and_expression() {
        let expr = FilterExpr::parse("math and add").unwrap();
        assert_eq!(
            expr,
            FilterExpr::And(
                Box::new(FilterExpr::Substring("math".into())),
                Box::new(FilterExpr::Substring("add".into())),
            )
        );
    }

    #[test]
    fn parse_or_expression() {
        let expr = FilterExpr::parse("math or utils").unwrap();
        assert_eq!(
            expr,
            FilterExpr::Or(
                Box::new(FilterExpr::Substring("math".into())),
                Box::new(FilterExpr::Substring("utils".into())),
            )
        );
    }

    #[test]
    fn parse_not_expression() {
        let expr = FilterExpr::parse("not slow").unwrap();
        assert_eq!(
            expr,
            FilterExpr::Not(Box::new(FilterExpr::Substring("slow".into()))),
        );
    }

    #[test]
    fn parse_grouped_expression() {
        let expr = FilterExpr::parse("(add or sub) and math").unwrap();
        assert_eq!(
            expr,
            FilterExpr::And(
                Box::new(FilterExpr::Or(
                    Box::new(FilterExpr::Substring("add".into())),
                    Box::new(FilterExpr::Substring("sub".into())),
                )),
                Box::new(FilterExpr::Substring("math".into())),
            )
        );
    }

    #[test]
    fn parse_and_binds_tighter_than_or() {
        let expr = FilterExpr::parse("a or b and c").unwrap();
        assert_eq!(
            expr,
            FilterExpr::Or(
                Box::new(FilterExpr::Substring("a".into())),
                Box::new(FilterExpr::And(
                    Box::new(FilterExpr::Substring("b".into())),
                    Box::new(FilterExpr::Substring("c".into())),
                )),
            )
        );
    }

    #[test]
    fn parse_chained_and() {
        let expr = FilterExpr::parse("a and b and c").unwrap();
        assert_eq!(
            expr,
            FilterExpr::And(
                Box::new(FilterExpr::And(
                    Box::new(FilterExpr::Substring("a".into())),
                    Box::new(FilterExpr::Substring("b".into())),
                )),
                Box::new(FilterExpr::Substring("c".into())),
            )
        );
    }

    #[test]
    fn parse_empty_input_errors() {
        assert!(FilterExpr::parse("").is_err());
    }

    #[test]
    fn parse_unmatched_paren_errors() {
        assert!(FilterExpr::parse("(add or sub").is_err());
    }

    // --- FilterExpr::matches tests ---

    #[test]
    fn expr_substring_matches_case_insensitive() {
        let expr = FilterExpr::Substring("ADD".into());
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(expr.matches(&test));
    }

    #[test]
    fn expr_substring_no_match() {
        let expr = FilterExpr::Substring("multiply".into());
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(!expr.matches(&test));
    }

    #[test]
    fn expr_and_matches() {
        let expr = FilterExpr::And(
            Box::new(FilterExpr::Substring("math".into())),
            Box::new(FilterExpr::Substring("add".into())),
        );
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(expr.matches(&test));
    }

    #[test]
    fn expr_or_matches() {
        let expr = FilterExpr::Or(
            Box::new(FilterExpr::Substring("utils".into())),
            Box::new(FilterExpr::Substring("add".into())),
        );
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(expr.matches(&test));
    }

    #[test]
    fn expr_not_matches() {
        let expr = FilterExpr::Not(Box::new(FilterExpr::Substring("slow".into())));
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(expr.matches(&test));
    }

    #[test]
    fn expr_not_excludes() {
        let expr = FilterExpr::Not(Box::new(FilterExpr::Substring("add".into())));
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(!expr.matches(&test));
    }

    // --- PathSpec tests ---

    #[test]
    fn pathspec_parse_file() {
        let spec = PathSpec::parse("tests/math.py").unwrap();
        assert_eq!(spec, PathSpec::File(PathBuf::from("tests/math.py")));
    }

    #[test]
    fn pathspec_parse_file_line() {
        let spec = PathSpec::parse("tests/math.py:10").unwrap();
        assert_eq!(spec, PathSpec::FileLine(PathBuf::from("tests/math.py"), 10));
    }

    #[test]
    fn pathspec_parse_invalid_line_errors() {
        // "tests/math.py:abc" — the `:abc` part doesn't parse as u32,
        // so the whole string is treated as a file path, not an error.
        let spec = PathSpec::parse("tests/math.py:abc").unwrap();
        assert_eq!(spec, PathSpec::File(PathBuf::from("tests/math.py:abc")));
    }

    #[test]
    fn pathspec_file_suffix_match() {
        let spec = PathSpec::File(PathBuf::from("math.py"));
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(spec.matches(&test));
    }

    #[test]
    fn pathspec_file_no_match() {
        let spec = PathSpec::File(PathBuf::from("utils.py"));
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(!spec.matches(&test));
    }

    #[test]
    fn pathspec_directory_matches_contained_tests() {
        let spec = PathSpec::File(PathBuf::from("tests"));
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(spec.matches(&test));
    }

    #[test]
    fn pathspec_nested_directory_matches_contained_tests() {
        let spec = PathSpec::File(PathBuf::from("tests/unit"));
        let test = make_test("test_add", "tests/unit/math.py", 10);
        assert!(spec.matches(&test));
    }

    #[test]
    fn pathspec_directory_does_not_match_sibling() {
        let spec = PathSpec::File(PathBuf::from("tests"));
        let test = make_test("test_add", "src/math.py", 10);
        assert!(!spec.matches(&test));
    }

    #[test]
    fn pathspec_file_line_matches() {
        let spec = PathSpec::FileLine(PathBuf::from("math.py"), 10);
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(spec.matches(&test));
    }

    #[test]
    fn pathspec_file_line_wrong_line() {
        let spec = PathSpec::FileLine(PathBuf::from("math.py"), 20);
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(!spec.matches(&test));
    }

    // --- TestFilter tests ---

    #[test]
    fn filter_from_args_empty() {
        let filter = TestFilter::from_args(&[], None, None).unwrap();
        assert!(filter.is_empty());
    }

    #[test]
    fn filter_from_args_with_paths() {
        let filter = TestFilter::from_args(&["tests/math.py".into()], None, None).unwrap();
        assert_eq!(filter.path_specs.len(), 1);
        assert!(filter.expr.is_none());
    }

    #[test]
    fn filter_from_args_with_expr() {
        let filter = TestFilter::from_args(&[], Some("test_add"), None).unwrap();
        assert!(filter.path_specs.is_empty());
        assert!(filter.expr.is_some());
    }

    #[test]
    fn filter_apply_paths_union() {
        let filter = TestFilter::from_args(
            &["tests/math.py".into(), "tests/utils.py".into()],
            None,
            None,
        )
        .unwrap();
        let tests = vec![
            make_test("test_add", "tests/math.py", 10),
            make_test("test_helper", "tests/utils.py", 5),
            make_test("test_other", "tests/other.py", 1),
        ];
        let filtered = filter.apply(tests);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn filter_apply_expr() {
        let filter = TestFilter::from_args(&[], Some("add"), None).unwrap();
        let tests = vec![
            make_test("test_add", "tests/math.py", 10),
            make_test("test_sub", "tests/math.py", 20),
        ];
        let filtered = filter.apply(tests);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "test_add");
    }

    #[test]
    fn filter_apply_path_and_expr_intersect() {
        let filter = TestFilter::from_args(&["tests/math.py".into()], Some("add"), None).unwrap();
        let tests = vec![
            make_test("test_add", "tests/math.py", 10),
            make_test("test_sub", "tests/math.py", 20),
            make_test("test_add", "tests/utils.py", 5),
        ];
        let filtered = filter.apply(tests);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "test_add");
        assert_eq!(
            filtered[0].file_path.as_deref(),
            Some(Path::new("tests/math.py"))
        );
    }

    #[test]
    fn filter_empty_passes_all() {
        let filter = TestFilter::from_args(&[], None, None).unwrap();
        let tests = vec![
            make_test("test_add", "tests/math.py", 10),
            make_test("test_sub", "tests/math.py", 20),
        ];
        let filtered = filter.apply(tests);
        assert_eq!(filtered.len(), 2);
    }

    #[test]
    fn expr_matches_against_test_id() {
        let expr = FilterExpr::Substring("math.py::test".into());
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(expr.matches(&test));
    }

    #[test]
    fn expr_matches_against_module_path() {
        let expr = FilterExpr::Substring("tests.math".into());
        let test = make_test("test_add", "tests/math.py", 10);
        assert!(expr.matches(&test));
    }

    #[test]
    fn pathspec_matches_test_without_file_path() {
        let spec = PathSpec::File(PathBuf::from("math.py"));
        let test = TestItem {
            name: "test_add".into(),
            module_path: "tests.math".into(),
            file_path: None,
            line_number: None,
            display_name: None,
            expected_assertions: vec![],
            ..Default::default()
        };
        assert!(!spec.matches(&test));
    }

    // --- matches_tags tests ---

    #[test]
    fn matches_tags_substring() {
        let expr = FilterExpr::Substring("slow".into());
        assert!(expr.matches_tags(&["slow".into(), "db".into()]));
        assert!(!expr.matches_tags(&["fast".into()]));
    }

    #[test]
    fn matches_tags_case_insensitive() {
        let expr = FilterExpr::Substring("SLOW".into());
        assert!(expr.matches_tags(&["slow".into()]));
    }

    #[test]
    fn matches_tags_and() {
        let expr = FilterExpr::And(
            Box::new(FilterExpr::Substring("slow".into())),
            Box::new(FilterExpr::Substring("db".into())),
        );
        assert!(expr.matches_tags(&["slow".into(), "db".into()]));
        assert!(!expr.matches_tags(&["slow".into()]));
    }

    #[test]
    fn matches_tags_not() {
        let expr = FilterExpr::Not(Box::new(FilterExpr::Substring("slow".into())));
        assert!(expr.matches_tags(&["fast".into()]));
        assert!(!expr.matches_tags(&["slow".into()]));
    }

    #[test]
    fn filter_with_markers_restricts_by_tags() {
        let filter = TestFilter::from_args(&[], None, Some("slow")).unwrap();
        let mut t1 = make_test("test_a", "tests/a.py", 1);
        t1.tags = vec!["slow".into()];
        let mut t2 = make_test("test_b", "tests/b.py", 1);
        t2.tags = vec!["fast".into()];
        let filtered = filter.apply(vec![t1, t2]);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "test_a");
    }

    #[test]
    fn filter_with_markers_is_not_empty() {
        let filter = TestFilter::from_args(&[], None, Some("slow")).unwrap();
        assert!(!filter.is_empty());
    }

    // --- group matching tests ---

    fn make_grouped_test(name: &str, groups: &[&str]) -> TestItem {
        TestItem {
            name: name.into(),
            module_path: "tests.math".into(),
            file_path: Some(PathBuf::from("tests/math.py")),
            line_number: Some(1),
            groups: groups.iter().map(|&s| s.into()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn expr_matches_group_name() {
        let expr = FilterExpr::Substring("Math".into());
        let test = make_grouped_test("test_add", &["Math", "addition"]);
        assert!(expr.matches(&test));
    }

    #[test]
    fn expr_matches_nested_group_name() {
        let expr = FilterExpr::Substring("addition".into());
        let test = make_grouped_test("test_add", &["Math", "addition"]);
        assert!(expr.matches(&test));
    }

    #[test]
    fn expr_matches_qualified_group_path() {
        let expr = FilterExpr::Substring("Math > addition".into());
        let test = make_grouped_test("test_add", &["Math", "addition"]);
        assert!(expr.matches(&test));
    }

    #[test]
    fn expr_no_match_wrong_group() {
        let expr = FilterExpr::Substring("subtraction".into());
        let test = make_grouped_test("test_add", &["Math", "addition"]);
        assert!(!expr.matches(&test));
    }

    #[test]
    fn filter_by_group_restricts_tests() {
        let filter = TestFilter::from_args(&[], Some("Math"), None).unwrap();
        let tests = vec![
            make_grouped_test("test_add", &["Math", "addition"]),
            make_test("test_standalone", "tests/other.py", 1),
        ];
        let filtered = filter.apply(tests);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].name, "test_add");
    }
}
