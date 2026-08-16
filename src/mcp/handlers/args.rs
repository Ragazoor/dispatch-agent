//! Single-declaration MCP argument boundary.
//!
//! Adding a field to a task-shaped MCP tool used to mean editing three
//! independent places — the hand-written JSON input schema, the `Deserialize`
//! args struct, and a long run of `if let Some(x) = parsed.x` mapping blocks —
//! none of which the compiler cross-checked. `deny_unknown_fields` turned any
//! disagreement between the first two into a runtime `-32602` rather than a
//! build error.
//!
//! [`mcp_args!`] collapses those three into one field list, in the same spirit
//! as `mcp_tools!` (which generates the tool registry) and `service_api!`
//! (which generates the service seam). Each field is declared once and expands
//! to all three surfaces, so a field cannot exist in the struct and be missing
//! from the schema or the mapping.
//!
//! ## Field grammar
//!
//! ```text
//! <serde attributes>
//! required|optional <name>: <type> = [<mode>] { <json schema for this field> };
//! ```
//!
//! `required` lists the field in the schema's `"required"` array; `optional`
//! does not. The mode says how the field reaches the service params builder:
//!
//! | Mode | Expands to |
//! |---|---|
//! | `[manual]` | nothing — the handler consumes the field itself |
//! | `[set(m)]` | `if let Some(v) = self.f { params = params.m(v) }` |
//! | `[set(m, conv)]` | same, with `conv(v)` |
//! | `[set_some(m)]` | same, with `Some(v)` — for setters that take an `Option` |
//!
//! Every mode is conditional on the field being present, which is what makes
//! "absent = leave untouched" the default. A setter that takes an `Option` and
//! would be handed `None` for an absent field is a no-op on a params value that
//! already defaults that field to `None`, so there is no unconditional mode.
//!
//! `[manual]` is the deliberate escape hatch for fields whose mapping isn't a
//! single setter call (a pair of fields that validate together, or an id that
//! is consumed constructing the builder). A `manual` field still appears in the
//! struct, the schema, `FIELD_NAMES` and `MANUAL_FIELDS` — only the mapping is
//! the handler's job. `MANUAL_FIELDS` exists so a coverage test can derive its
//! own exclusion set rather than hand-keeping a list that drifts.
//!
//! ## Prerequisite: a builder-shaped params type
//!
//! Every mode emits `params = params.setter(…)`, so a tool can only adopt
//! `mcp_args!` if its service params type has chained setters. `UpdateTaskParams`
//! does; `UpdateEpicParams` and `CreateTaskParams` are plain structs built by
//! literal, so converting those tools needs either service-layer builders or a
//! new struct-literal mode first. That is a design step, not a one-line one —
//! don't budget for it as a rename.

/// Generate an MCP args struct, its JSON input schema, and its arg→params
/// mapping from one field list. See the module docs for the grammar.
macro_rules! mcp_args {
    (
        $(#[$smeta:meta])*
        $vis:vis struct $name:ident;
        schema fn $schema_fn:ident;
        apply fn $apply_fn:ident($params:ty);

        $(
            $(#[$fmeta:meta])*
            // `ident`, not `tt`: a `tt` here is locally ambiguous with the
            // `#[...]` of the attribute repetition above it.
            $req:ident $fname:ident: $fty:ty = $mode:tt $fschema:tt;
        )+
    ) => {
        $(#[$smeta])*
        #[derive(::serde::Deserialize)]
        #[serde(deny_unknown_fields)]
        $vis struct $name {
            $(
                $(#[$fmeta])*
                $vis $fname: $fty,
            )+
        }

        impl $name {
            /// Wire field names, in declaration order. Shared by the schema and
            /// the struct by construction, so a parity test over this list is
            /// checking the generation, not two hand-kept copies.
            // Introspection for the boundary parity tests; nothing in the
            // handler path reads it.
            #[allow(dead_code)]
            $vis const FIELD_NAMES: &'static [&'static str] = &[$(stringify!($fname)),+];

            /// The subset of [`Self::FIELD_NAMES`] declared `[manual]`, i.e. the
            /// fields the generated mapping deliberately does not apply. A
            /// coverage test derives its exclusions from this rather than
            /// hardcoding names, so adding a `[manual]` field cannot quietly
            /// widen what the test forgives.
            #[allow(dead_code)]
            $vis fn manual_fields() -> ::std::vec::Vec<&'static str> {
                [$( mcp_args!(@manual_name $mode, $fname) ),+]
                    .into_iter()
                    .flatten()
                    .collect()
            }

            /// Fold every non-`manual` field into the service params builder.
            #[allow(unused_mut)]
            $vis fn $apply_fn(self, params: $params) -> $params {
                let mut params = params;
                $( mcp_args!(@apply $mode, params, self, $fname); )+
                params
            }
        }

        /// The tool's JSON input schema, generated from the same field list as
        /// the args struct above.
        $vis fn $schema_fn() -> ::serde_json::Value {
            let required: ::std::vec::Vec<&'static str> =
                [$( mcp_args!(@required $req, $fname) ),+]
                    .into_iter()
                    .flatten()
                    .collect();
            ::serde_json::json!({
                "type": "object",
                "properties": {
                    $( (stringify!($fname)): $fschema ),+
                },
                "required": required
            })
        }
    };

    (@required required, $fname:ident) => { ::std::option::Option::Some(stringify!($fname)) };
    (@required optional, $fname:ident) => { ::std::option::Option::<&'static str>::None };

    // `[manual]` first, then a catch-all: macro_rules tries rules in order.
    (@manual_name [manual], $fname:ident) => { ::std::option::Option::Some(stringify!($fname)) };
    (@manual_name $other:tt, $fname:ident) => { ::std::option::Option::<&'static str>::None };

    (@apply [manual], $params:ident, $self:ident, $fname:ident) => {};
    (@apply [set($setter:ident)], $params:ident, $self:ident, $fname:ident) => {
        mcp_args!(@apply [set($setter, |v| v)], $params, $self, $fname)
    };
    (@apply [set_some($setter:ident)], $params:ident, $self:ident, $fname:ident) => {
        mcp_args!(@apply [set($setter, Some)], $params, $self, $fname)
    };
    (@apply [set($setter:ident, $conv:expr)], $params:ident, $self:ident, $fname:ident) => {
        if let Some(v) = $self.$fname {
            $params = $params.$setter(($conv)(v));
        }
    };
}
