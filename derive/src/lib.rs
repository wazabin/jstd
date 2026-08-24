use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, Type, parse_macro_input};

/// Which primitive integer backs an `Identifier` tuple struct.
enum Backing {
    Usize,
    U32,
}

#[proc_macro_derive(Identifier)]
pub fn derive_identifier(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = input.ident;

    let backing = match input.data {
        Data::Struct(data) => match data.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                match fields.unnamed.first().map(|f| &f.ty) {
                    Some(Type::Path(type_path)) if type_path.path.is_ident("usize") => {
                        Some(Backing::Usize)
                    }
                    Some(Type::Path(type_path)) if type_path.path.is_ident("u32") => {
                        Some(Backing::U32)
                    }
                    _ => None,
                }
            }
            _ => None,
        },
        _ => None,
    };

    let Some(backing) = backing else {
        return syn::Error::new_spanned(
            name,
            "Identifier can only be derived for tuple structs with a single `usize` or `u32` field, e.g. struct MyId(usize);",
        )
        .to_compile_error()
        .into();
    };

    // `usize`-backed IDs keep their original, cast-free codegen so downstream
    // clippy (`-D warnings`) never sees a no-op `as usize`. `u32`-backed IDs
    // (the function-local IR IDs) cast to/from the `usize` lingua franca of
    // `Registry`.
    let (new_body, from_usize_body, into_usize_body, ser_body, de_body) = match backing {
        Backing::Usize => (
            quote! { Self(id) },
            quote! { Self(value) },
            quote! { id.0 },
            quote! { self.0 as u64 },
            quote! { Self(id as usize) },
        ),
        Backing::U32 => (
            quote! { Self(id as u32) },
            quote! { Self(value as u32) },
            quote! { id.0 as usize },
            quote! { self.0 as u64 },
            quote! { Self(id as u32) },
        ),
    };

    TokenStream::from(quote! {
        impl #name {
            /// Creates an identifier from its index.
            pub const fn new(id: usize) -> Self {
                #new_body
            }
        }

        impl ::core::marker::Copy for #name {}

        impl ::core::clone::Clone for #name {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl ::core::default::Default for #name {
            fn default() -> Self {
                Self(0)
            }
        }

        impl ::core::cmp::PartialEq for #name {
            fn eq(&self, other: &Self) -> bool {
                self.0 == other.0
            }
        }

        impl ::core::cmp::Eq for #name {}

        impl ::core::cmp::PartialOrd for #name {
            fn partial_cmp(&self, other: &Self) -> ::core::option::Option<::core::cmp::Ordering> {
                ::core::option::Option::Some(::core::cmp::Ord::cmp(self, other))
            }
        }

        impl ::core::cmp::Ord for #name {
            fn cmp(&self, other: &Self) -> ::core::cmp::Ordering {
                ::core::cmp::Ord::cmp(&self.0, &other.0)
            }
        }

        impl ::core::hash::Hash for #name {
            fn hash<H: ::core::hash::Hasher>(&self, state: &mut H) {
                ::core::hash::Hash::hash(&self.0, state);
            }
        }

        impl ::core::fmt::Debug for #name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_tuple(stringify!(#name)).field(&self.0).finish()
            }
        }

        impl ::core::fmt::Display for #name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<usize> for #name {
            fn from(value: usize) -> Self {
                #from_usize_body
            }
        }

        impl From<#name> for usize {
            fn from(id: #name) -> usize {
                #into_usize_body
            }
        }


        impl ::jstd::registry::Identifier for #name {}

        impl ::serde::Serialize for #name {
            fn serialize<S>(&self, serializer: S) -> ::core::result::Result<S::Ok, S::Error>
            where
                S: ::serde::Serializer,
            {
                ::serde::Serialize::serialize(&(#ser_body), serializer)
            }
        }

        impl<'de> ::serde::Deserialize<'de> for #name {
            fn deserialize<D>(deserializer: D) -> ::core::result::Result<Self, D::Error>
            where
                D: ::serde::Deserializer<'de>,
            {
                let id = <u64 as ::serde::Deserialize<'de>>::deserialize(deserializer)?;
                Ok(#de_body)
            }
        }
    })
}
