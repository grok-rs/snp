//! Contract test (ARCHITECTURE §1.3 "Contract tests"): references every public item of §1.4.6 with
//! explicit types, so signature drift breaks compilation, plus the `Send + Sync` assertions. Stub
//! functions are only coerced to `fn` pointers (never called), so the test passes on W0 stubs.
//!
//! Owner: WP-14 (created by WP-00).

#[cfg(test)]
mod api {
    use std::borrow::Cow;
    use std::fmt::Display;

    use snp_pycompat::codec::Utf8DecodeError;
    use snp_yaml::emit::{self, Quote};
    use snp_yaml::{
        Document, ImplicitTag, Limits, Mark, Node, NodeId, NodeKind, ScalarStyle, Schema, YamlError,
    };

    fn send_sync<T: Send + Sync>() {}
    fn display<T: Display + Clone>() {}

    /// Type-checks `t` against the explicit type argument.
    fn has<T>(t: T) {
        drop(t);
    }

    #[test]
    fn send_sync_types() {
        send_sync::<Limits>();
        send_sync::<Schema>();
        send_sync::<YamlError>();
        send_sync::<Mark>();
        send_sync::<NodeId>();
        send_sync::<NodeKind>();
        send_sync::<ScalarStyle>();
        send_sync::<Node>();
        send_sync::<Document>();
        send_sync::<Quote>();
        send_sync::<ImplicitTag>();
        display::<YamlError>();
    }

    #[test]
    fn types() {
        let Limits {
            max_depth,
            max_constructed_nodes,
            max_bytes,
        }: Limits = Limits::default();
        has::<(u32, u32, usize)>((max_depth, max_constructed_nodes, max_bytes));
        has::<[Schema; 2]>([Schema::PyYaml11Safe, Schema::Yaml12Core]);
        let start = Mark {
            index: 0,
            line: 0,
            column: 0,
        };
        let node = Node {
            kind: NodeKind::Scalar("x".into()),
            tag: "tag:yaml.org,2002:str".into(),
            style: ScalarStyle::Plain,
            span: 0..1,
            start,
            anchor: None,
        };
        let doc = Document {
            nodes: vec![node],
            root: Some(NodeId(0)),
        };
        has::<u32>(NodeId(0).0);
        has::<[NodeKind; 2]>([
            NodeKind::Seq(vec![NodeId(0)]),
            NodeKind::Map(vec![(NodeId(0), NodeId(0))]),
        ]);
        has::<[ScalarStyle; 5]>([
            ScalarStyle::Single,
            ScalarStyle::Double,
            ScalarStyle::Literal,
            ScalarStyle::Folded,
            ScalarStyle::NotScalar,
        ]);
        has::<[Quote; 3]>([Quote::Plain, Quote::Single, Quote::Double]);
        has::<[ImplicitTag; 8]>([
            ImplicitTag::Null,
            ImplicitTag::Bool,
            ImplicitTag::Int,
            ImplicitTag::Float,
            ImplicitTag::Timestamp,
            ImplicitTag::Merge,
            ImplicitTag::Value,
            ImplicitTag::Str,
        ]);
        has::<String>(format!("{doc:?}"));
    }

    #[test]
    fn functions() {
        has::<fn(&[u8]) -> Result<Cow<'_, str>, Utf8DecodeError>>(snp_yaml::decode_text);
        has::<fn(&str, Schema, &Limits) -> Result<Document, YamlError>>(snp_yaml::compose);
        has::<fn(&Document, Schema, &Limits) -> Result<cfgv::Value, YamlError>>(
            snp_yaml::construct,
        );
        has::<fn(&str, &Limits) -> Result<cfgv::Value, YamlError>>(snp_yaml::load);
        has::<fn(&cfgv::Value) -> String>(emit::dump);
        has::<fn(&str, Quote) -> String>(emit::scalar_for_rev);
        has::<fn(&str, Schema) -> ImplicitTag>(snp_yaml::resolve_plain);
    }

    /// Additive items beyond ARCHITECTURE §1.4.6 (WP-14): multi-document compose, the Python exception class of an
    /// error, the `yaml` implicit tag and its tag string, and the identity-preserving re-dump of autoupdate.
    #[test]
    fn additive_items() {
        has::<fn(&str, Schema, &Limits) -> Result<Vec<Document>, YamlError>>(snp_yaml::compose_all);
        has::<fn(&YamlError) -> &'static str>(YamlError::py_type);
        has::<fn(ImplicitTag) -> &'static str>(ImplicitTag::as_tag);
        has::<ImplicitTag>(ImplicitTag::Yaml);
        has::<fn(&str, &Limits) -> Result<String, YamlError>>(emit::redump);
    }
}
