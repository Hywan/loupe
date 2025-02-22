use proc_macro::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::{parse, Data, DataEnum, DataStruct, DeriveInput, Fields, Generics, Ident, Index};

#[proc_macro_derive(MemoryUsage)]
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
    let lifetimes_and_generics = &generics.params;
    let where_clause = &generics.where_clause;

    let sum = join_fold(
        // Check all fields of the `struct`.
        match &data.fields {
            // Field has the form:
            //
            //     F { x, y }
            Fields::Named(ref fields) => fields
                .named
                .iter()
                .map(|field| {
                    let ident = field.ident.as_ref().unwrap();
                    let span = ident.span();

                    quote_spanned!(
                        span => MemoryUsage::size_of_val(&self.#ident, visited) - std::mem::size_of_val(&self.#ident)
                    )
                })
                .collect(),

            // Field has the form:
            //
            //     F
            Fields::Unit => vec![],

            // Field has the form:
            //
            //     F(x, y)
            Fields::Unnamed(ref fields) => fields
                .unnamed
                .iter()
                .enumerate()
                .map(|(nth, _field)| {
                    let ident = Index::from(nth);

                    quote! { MemoryUsage::size_of_val(&self.#ident, visited) - std::mem::size_of_val(&self.#ident) }
                })
                .collect(),
        }
        .into_iter(),
        |x, y| quote! { #x + #y },
        quote! { 0 },
    );

    // Implement the `MemoryUsage` trait for `struct_name`.
    (quote! {
        #[allow(dead_code)]
        impl < #lifetimes_and_generics > MemoryUsage for #struct_name < #lifetimes_and_generics >
        #where_clause
        {
            fn size_of_val(&self, visited: &mut MemoryUsageTracker) -> usize {
                std::mem::size_of_val(self) + #sum
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
    let lifetimes_and_generics = &generics.params;
    let where_clause = &generics.where_clause;

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
                let (pattern, sum) = match variant.fields {
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
                                    MemoryUsage::size_of_val(#ident, visited) - std::mem::size_of_val(#ident)
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
                                    MemoryUsage::size_of_val(#ident, visited) - std::mem::size_of_val(#ident)
                                }),
                                |x, y| quote! { #x + #y },
                                quote! { 0 },
                            );

                            quote! { #sum }
                        };

                        (pattern, sum)
                    }
                };

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
        impl < #lifetimes_and_generics > MemoryUsage for #enum_name < #lifetimes_and_generics >
        #where_clause
        {
            fn size_of_val(&self, visited: &mut MemoryUsageTracker) -> usize {
                std::mem::size_of_val(self) + match self {
                    #match_arms
                }
            }
        }
    })
    .into()
}
