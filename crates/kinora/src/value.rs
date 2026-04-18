use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

pub type Metadata = BTreeMap<String, Value>;

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn as_str(&self) -> Option<&str> {
        if let Value::String(s) = self { Some(s) } else { None }
    }

    pub fn as_bool(&self) -> Option<bool> {
        if let Value::Bool(b) = self { Some(*b) } else { None }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        if let Value::Array(a) = self { Some(a) } else { None }
    }

    pub fn as_object(&self) -> Option<&BTreeMap<String, Value>> {
        if let Value::Object(o) = self { Some(o) } else { None }
    }
}

/// Merge metadata using per-field latest-wins. Fields present in `newer`
/// overwrite those in `older`. A `Value::Null` in `newer` removes the field.
pub fn merge_metadata(older: Metadata, newer: Metadata) -> Metadata {
    let mut out = older;
    for (k, v) in newer {
        if v.is_null() {
            out.remove(&k);
        } else {
            out.insert(k, v);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::String(v.to_owned())
    }

    #[test]
    fn null_recognition() {
        assert!(Value::Null.is_null());
        assert!(!s("x").is_null());
    }

    #[test]
    fn accessors_return_inner() {
        assert_eq!(s("hi").as_str(), Some("hi"));
        assert_eq!(Value::Bool(true).as_bool(), Some(true));
        assert!(s("x").as_bool().is_none());
    }

    #[test]
    fn merge_overlays_new_fields() {
        let mut older = Metadata::new();
        older.insert("name".into(), s("old-name"));
        let mut newer = Metadata::new();
        newer.insert("name".into(), s("new-name"));
        let out = merge_metadata(older, newer);
        assert_eq!(out.get("name").unwrap().as_str(), Some("new-name"));
    }

    #[test]
    fn merge_preserves_untouched_fields() {
        let mut older = Metadata::new();
        older.insert("name".into(), s("foo"));
        older.insert("title".into(), s("bar"));
        let mut newer = Metadata::new();
        newer.insert("title".into(), s("baz"));
        let out = merge_metadata(older, newer);
        assert_eq!(out.get("name").unwrap().as_str(), Some("foo"));
        assert_eq!(out.get("title").unwrap().as_str(), Some("baz"));
    }

    #[test]
    fn null_removes_field() {
        let mut older = Metadata::new();
        older.insert("draft".into(), Value::Bool(true));
        let mut newer = Metadata::new();
        newer.insert("draft".into(), Value::Null);
        let out = merge_metadata(older, newer);
        assert!(!out.contains_key("draft"));
    }

    #[test]
    fn merge_is_wholesale_for_arrays() {
        let mut older = Metadata::new();
        older.insert("tags".into(), Value::Array(vec![s("a"), s("b")]));
        let mut newer = Metadata::new();
        newer.insert("tags".into(), Value::Array(vec![s("c")]));
        let out = merge_metadata(older, newer);
        assert_eq!(
            out.get("tags").unwrap().as_array().unwrap(),
            &[s("c")]
        );
    }
}
