//! Companion of the [`loupe`](../loupe-derive/index.html) crate.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{format_ident, quote, quote_spanned, ToTokens};
use syn::{
    parse, Attribute, Data, DataEnum, DataStruct, DeriveInput, Fields, Generics, Ident, Index,
};

/// Procedural macro to implement the `loupe::MemoryUsage` trait
/// automatically for structs and enums.
///
/// All struct fields and enum variants must implement `MemoryUsage`
/// trait. If it's not possible, the `#[loupe(skip)]` attribute can be
/// used on a field or a variant to instruct the derive procedural
/// macro to skip that item.
///
/// # Example
///
/// ```rust,ignore
/// #[derive(MemoryUsage)]
/// struct Point {
///     x: i32,
///     y: i32,
/// }
///
/// struct Mystery { ptr: *const i32 }
///
/// #[derive(MemoryUsage)]
/// struct S {
///     points: Vec<Point>,
///
///     #[loupe(skip)]
///     other: Mystery,
/// }
/// ```
#[proc_macro_derive(MemoryUsage, attributes(loupe))]
pub fn derive_memory_usage(input: TokenStream) -> TokenStream {
    let derive_input: DeriveInput = parse(input).unwrap();

    match derive_input.data {
        Data::Struct(ref struct_data) => {
            derive_memory_usage_for_struct(&derive_input.ident, struct_data, &derive_input.generics)
        }

        Data::Enum(ref enum_data) => {
            derive_memory_usage_for_enum(&derive_input.ident, enum_data, &derive_input.generics)
        }

        Data::Union(_) => panic!("unions are not yet implemented"),
        /*
        // TODO: unions.
        // We have no way of knowing which union member is active, so we should
        // refuse to derive an impl except for unions where all members are
        // primitive types or arrays of them.
        Data::Union(ref union_data) => {
            derive_memory_usage_union(union_data)
        },
        */
    }
}

// TODO: use Iterator::fold_first once it's stable. https://github.com/rust-lang/rust/pull/79805
fn join_fold<I, F, B>(mut iter: I, function: F, empty: B) -> B
where
    I: Iterator<Item = B>,
    F: FnMut(B, I::Item) -> B,
{
    if let Some(first) = iter.next() {
        iter.fold(first, function)
    } else {
        empty
    }
}

fn derive_memory_usage_for_struct(
    struct_name: &Ident,
    data: &DataStruct,
    generics: &Generics,
) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let field_names: Vec<IdentOrIndex> = vec![];
    let (size, field_names) = join_fold(
        // Check all fields of the `struct`.
        match &data.fields {
            // Field has a name, like in:
            //
            //     F { x, y }
            Fields::Named(ref fields) => fields
                .named
                .iter()
                .filter_map(|field| {
                    if must_skip(&field.attrs) {
                        return None;
                    }

                    let ident = field.ident.as_ref().unwrap();
                    let span = ident.span();

                    Some((
                        quote_spanned!(
                            span => loupe::MemoryUsage::size_of_val(&self.#ident, visited) - std::mem::size_of_val(&self.#ident)
                        ),
                        vec![IdentOrIndex::Ident(ident.clone())],
                    ))
                })
                .collect(),

            // Field is a unit, like in:
            //
            //     F
            Fields::Unit => vec![],

            // Field has no name, like in:
            //
            //     F(x, y)
            Fields::Unnamed(ref fields) => fields
                .unnamed
                .iter()
                .enumerate()
                .filter_map(|(nth, field)| {
                    if must_skip(&field.attrs) {
                        return None;
                    }

                    let ident = Index::from(nth);

                    Some((
                        quote! { loupe::MemoryUsage::size_of_val(&self.#ident, visited) - std::mem::size_of_val(&self.#ident) },
                        vec![IdentOrIndex::Index(ident)],
                    ))
                })
                .collect(),
        }
        .into_iter(),
        |(acc_size, mut acc_field_idents), (size, mut field_ident)| (quote! { #acc_size + #size }, { acc_field_idents.append(&mut field_ident); acc_field_idents }),
        (quote! { 0 }, field_names),
    );

    // Implement the `MemoryUsage` trait for `struct_name`.
    (quote! {
        #[allow(dead_code)]
        impl #impl_generics loupe::MemoryUsage for #struct_name #ty_generics
        #where_clause
        {
            fn size_of_val(&self, visited: &mut loupe::MemoryUsageTracker) -> usize {
                std::mem::size_of_val(self) + #size
            }

            fn graph_size_of_val(
                &self,
                grapher: &mut loupe::MemoryUsageGrapher,
                tracker: &mut loupe::MemoryUsageTracker
            ) -> std::io::Result<()> {
                let struct_name = stringify!(#struct_name);
                let fields: &str = concat!(#( " | <", stringify!(#field_names), "> ", stringify!(#field_names) ),*);
                let links: &str = concat!(
                    #( "\"", stringify!(#struct_name), "\":", stringify!(#field_names), " -> node???:??? [ label = \"...\" ]\n" ),*
                );

                grapher.writer().write_all(&format!(
                    r#""{struct_name}" [
  label = "<0> {struct_name}{fields}"
  shape = "record"
]

{links}
"#,
                    struct_name = struct_name,
                    fields = fields,
                    links = links,
                ).as_bytes())?;

                Ok(())
            }
        }
    })
    .into()
}

fn derive_memory_usage_for_enum(
    enum_name: &Ident,
    data: &DataEnum,
    generics: &Generics,
) -> TokenStream {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let match_arms = join_fold(
        data.variants
            .iter()
            .map(|variant| {
                let ident = &variant.ident;
                let span = ident.span();

                // Check all the variants of the `enum`.
                //
                // We want to generate something like this:
                //
                //     Self::Variant ... => { ... }
                //           ^^^^^^^ ^^^      ^^^
                //           |       |        |
                //           |       |        given by the `sum` variable
                //           |       given by the `pattern` variable
                //           given by the `ident` variable
                //
                // Let's compute the `pattern` and `sum` parts.
                let (pattern, mut sum) = match variant.fields {
                    // Variant has the form:
                    //
                    //     V { x, y }
                    //
                    // We want to generate:
                    //
                    //     Self::V { x, y } => { /* memory usage of x + y */ }
                    Fields::Named(ref fields) => {
                        // Collect the identifiers.
                        let identifiers = fields.named.iter().map(|field| {
                            let ident = field.ident.as_ref().unwrap();
                            let span = ident.span();

                            quote_spanned!(span => #ident)
                        });

                        // Generate the `pattern` part.
                        let pattern = {
                            let pattern = join_fold(
                                identifiers.clone(),
                                |x, y| quote! { #x , #y },
                                quote! {}
                            );

                            quote! { { #pattern } }
                        };

                        // Generate the `sum` part.
                        let sum = {
                            let sum = join_fold(
                                identifiers.map(|ident| quote! {
                                    loupe::MemoryUsage::size_of_val(#ident, visited) - std::mem::size_of_val(#ident)
                                }),
                                |x, y| quote! { #x + #y },
                                quote! { 0 },
                            );

                            quote! { #sum }
                        };

                        (pattern, sum)
                    }

                    // Variant has the form:
                    //
                    //     V
                    //
                    // We want to generate:
                    //
                    //     Self::V => { 0 }
                    Fields::Unit => {
                        let pattern = quote! {};
                        let sum = quote! { 0 };

                        (pattern, sum)
                    },

                    // Variant has the form:
                    //
                    //     V(x, y)
                    //
                    // We want to generate:
                    //
                    //     Self::V(x, y) => { /* memory usage of x + y */ }
                    Fields::Unnamed(ref fields) => {
                        // Collect the identifiers. They are unnamed,
                        // so let's use the `xi` convention where `i`
                        // is the identifier index.
                        let identifiers = fields
                            .unnamed
                            .iter()
                            .enumerate()
                            .map(|(nth, _field)| {
                                let ident = format_ident!("x{}", Index::from(nth));

                                quote! { #ident }
                            });

                        // Generate the `pattern` part.
                        let pattern = {
                            let pattern = join_fold(
                                identifiers.clone(),
                                |x, y| quote! { #x , #y },
                                quote! {}
                            );

                            quote! { ( #pattern ) }
                        };

                        // Generate the `sum` part.
                        let sum = {
                            let sum = join_fold(
                                identifiers.map(|ident| quote! {
                                    loupe::MemoryUsage::size_of_val(#ident, visited) - std::mem::size_of_val(#ident)
                                }),
                                |x, y| quote! { #x + #y },
                                quote! { 0 },
                            );

                            quote! { #sum }
                        };

                        (pattern, sum)
                    }
                };

                if must_skip(&variant.attrs) {
                    sum = quote! { 0 };
                }

                // At this step, `pattern` and `sum` are well
                // defined. Let's generate the full arm for the
                // `match` statement.
                quote_spanned! { span => Self::#ident#pattern => #sum }
            }
        ),
        |x, y| quote! { #x , #y },
        quote! {},
    );

    // Implement the `MemoryUsage` trait for `enum_name`.
    (quote! {
        #[allow(dead_code)]
        impl #impl_generics loupe::MemoryUsage for #enum_name #ty_generics
        #where_clause
        {
            fn size_of_val(&self, visited: &mut loupe::MemoryUsageTracker) -> usize {
                std::mem::size_of_val(self) + match self {
                    #match_arms
                }
            }
        }
    })
    .into()
}

fn must_skip(attrs: &[Attribute]) -> bool {
    attrs.iter().any(|attr| {
        attr.path.is_ident("loupe") && matches!(attr.parse_args::<Ident>(), Ok(a) if a == "skip")
    })
}

enum IdentOrIndex {
    Ident(Ident),
    Index(Index),
}

impl ToTokens for IdentOrIndex {
    fn to_tokens(&self, tokens: &mut TokenStream2) {
        match self {
            Self::Ident(ident) => ident.to_tokens(tokens),
            Self::Index(index) => index.to_tokens(tokens),
        }
    }
}
