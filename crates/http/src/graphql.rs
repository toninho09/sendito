//! GraphQL introspection and schema navigation.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Introspection requests include deprecated fields and six wrapper levels.
pub const INTROSPECTION_QUERY: &str = r#"query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types {
      kind name description
      fields(includeDeprecated: true) {
        name description
        args { name description type { ...TypeRef } defaultValue }
        type { ...TypeRef }
      }
      inputFields { name description type { ...TypeRef } defaultValue }
      possibleTypes { name }
    }
  }
}
fragment TypeRef on __Type {
  kind name ofType { kind name ofType { kind name ofType {
    kind name ofType { kind name ofType { kind name ofType { kind name } } }
  } } }
}"#;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlTypeRef {
    pub kind: String,
    pub name: Option<String>,
    pub of_type: Option<Box<GqlTypeRef>>,
}

impl GqlTypeRef {
    pub fn named(&self) -> Option<&str> {
        self.name
            .as_deref()
            .or_else(|| self.of_type.as_deref()?.named())
    }

    pub fn label(&self) -> String {
        match self.kind.as_str() {
            "NON_NULL" => format!(
                "{}!",
                self.of_type.as_deref().map(Self::label).unwrap_or_default()
            ),
            "LIST" => format!(
                "[{}]",
                self.of_type.as_deref().map(Self::label).unwrap_or_default()
            ),
            _ => self.name.clone().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GqlField {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "type")]
    pub ty: GqlTypeRef,
    #[serde(default)]
    pub args: Vec<GqlField>,
    #[serde(rename = "defaultValue")]
    pub default_value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GqlType {
    pub kind: String,
    pub name: String,
    pub description: Option<String>,
    pub fields: Option<Vec<GqlField>>,
    pub input_fields: Option<Vec<GqlField>>,
    pub possible_types: Option<Vec<GqlNamed>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GqlNamed {
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct GraphqlSchema {
    pub query_type: String,
    pub mutation_type: Option<String>,
    pub subscription_type: Option<String>,
    pub types: BTreeMap<String, GqlType>,
}

impl GraphqlSchema {
    pub fn from_json(bytes: &[u8]) -> Result<Self, String> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Schema {
            query_type: GqlNamed,
            mutation_type: Option<GqlNamed>,
            subscription_type: Option<GqlNamed>,
            types: Vec<GqlType>,
        }
        let document: serde_json::Value = serde_json::from_slice(bytes)
            .map_err(|_| "invalid GraphQL introspection JSON".to_string())?;
        if document
            .get("errors")
            .and_then(|e| e.as_array())
            .is_some_and(|e| !e.is_empty())
        {
            // Server error text can contain credentials. Do not include it.
            return Err("GraphQL introspection returned errors".into());
        }
        let value = document
            .pointer("/data/__schema")
            .filter(|v| v.is_object())
            .ok_or("GraphQL introspection has no schema")?;
        let schema: Schema = serde_json::from_value(value.clone())
            .map_err(|_| "invalid GraphQL introspection schema".to_string())?;
        let types: BTreeMap<_, _> = schema
            .types
            .into_iter()
            .map(|t| (t.name.clone(), t))
            .collect();
        if !types.contains_key(&schema.query_type.name) {
            return Err("GraphQL introspection has no query root type".into());
        }
        Ok(Self {
            query_type: schema.query_type.name,
            mutation_type: schema.mutation_type.map(|t| t.name),
            subscription_type: schema.subscription_type.map(|t| t.name),
            types,
        })
    }

    pub fn root(&self, operation: &str) -> Option<&GqlType> {
        let name = match operation {
            "mutation" => self.mutation_type.as_deref()?,
            "subscription" => self.subscription_type.as_deref()?,
            _ => &self.query_type,
        };
        self.types.get(name)
    }

    pub fn selection_type<'a>(&'a self, operation: &str, path: &[&str]) -> Option<&'a GqlType> {
        let mut ty = self.root(operation)?;
        for name in path {
            let field = ty.fields.as_ref()?.iter().find(|f| f.name == *name)?;
            ty = self.types.get(field.ty.named()?)?;
        }
        Some(ty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graphql_schema_navigation() {
        let schema = GraphqlSchema::from_json(br#"{"data":{"__schema":{
          "queryType":{"name":"Query"}, "mutationType":{"name":"Mutation"},
          "types":[
            {"kind":"OBJECT","name":"Query","fields":[
              {"name":"heroes","args":[{"name":"id","type":{"kind":"SCALAR","name":"ID"}}],
               "type":{"kind":"NON_NULL","ofType":{"kind":"LIST","ofType":{"kind":"OBJECT","name":"Hero"}}}}
            ]},
            {"kind":"OBJECT","name":"Mutation","fields":[]},
            {"kind":"OBJECT","name":"Hero","fields":[{"name":"name","type":{"kind":"SCALAR","name":"String"}}]},
            {"kind":"INTERFACE","name":"Character","possibleTypes":[{"name":"Hero"}]}
          ]
        }}}"#).unwrap();
        let root = schema.root("query").unwrap();
        let field = &root.fields.as_ref().unwrap()[0];
        assert_eq!(field.ty.label(), "[Hero]!");
        assert_eq!(field.args[0].name, "id");
        assert_eq!(
            schema.selection_type("query", &["heroes"]).unwrap().name,
            "Hero"
        );
        assert!(schema.selection_type("query", &["absent"]).is_none());
        assert!(schema.root("subscription").is_none());
        assert_eq!(schema.root("mutation").unwrap().name, "Mutation");
        assert_eq!(
            schema.types["Character"].possible_types.as_ref().unwrap()[0].name,
            "Hero"
        );
    }

    #[test]
    fn graphql_schema_rejects_errors_and_missing_data() {
        for bytes in [
            b"invalid".as_slice(),
            b"{}",
            br#"{"data":{"__schema":null}}"#,
            br#"{"errors":[{"message":"secret"}]}"#,
            br#"{"data":{"__schema":{"queryType":{"name":"Missing"},"types":[]}}}"#,
        ] {
            let error = GraphqlSchema::from_json(bytes).unwrap_err();
            assert!(!error.contains("secret"));
        }
    }
}
