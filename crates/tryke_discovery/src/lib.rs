use std::path::PathBuf;

use tryke_types::TestItem;

#[must_use]
pub fn discover() -> Vec<TestItem> {
    vec![
        TestItem {
            name: "test_add".into(),
            module_path: "tests.math".into(),
            file_path: Some(PathBuf::from("tests/math.py")),
            line_number: Some(10),
        },
        TestItem {
            name: "test_sub".into(),
            module_path: "tests.math".into(),
            file_path: Some(PathBuf::from("tests/math.py")),
            line_number: Some(25),
        },
        TestItem {
            name: "test_parse".into(),
            module_path: "tests.parser".into(),
            file_path: Some(PathBuf::from("tests/parser.py")),
            line_number: Some(8),
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discover_returns_expected_items() {
        let items = discover();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].name, "test_add");
        assert_eq!(items[0].module_path, "tests.math");
        assert_eq!(items[1].name, "test_sub");
        assert_eq!(items[2].name, "test_parse");
        assert_eq!(items[2].module_path, "tests.parser");
    }
}
