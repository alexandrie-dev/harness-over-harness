//! Typed dependency and plugin declarations for `hoh-kernel`.
use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, spanned::Spanned};

fn wrapped(ty: &syn::Type, name: &str) -> Option<syn::Type> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let seg = path.path.segments.last()?;
    if seg.ident != name {
        return None;
    }
    let syn::PathArguments::AngleBracketed(args) = &seg.arguments else {
        return None;
    };
    if args.args.len() != 1 {
        return None;
    }
    match args.args.first()? {
        syn::GenericArgument::Type(ty) => Some(ty.clone()),
        _ => None,
    }
}
#[proc_macro_derive(Inject)]
pub fn inject(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as syn::DeriveInput);
    let result = (|| {
        let syn::Data::Struct(data) = &input.data else {
            return Err(syn::Error::new(
                input.span(),
                "Inject requires a struct with named Service<K> fields",
            ));
        };
        let syn::Fields::Named(fields) = &data.fields else {
            return Err(syn::Error::new(
                input.span(),
                "Inject requires named Service<K> fields",
            ));
        };
        let mut names = Vec::new();
        let mut keys = Vec::new();
        for field in &fields.named {
            let Some(key) = wrapped(&field.ty, "Service") else {
                return Err(syn::Error::new(
                    field.ty.span(),
                    "Inject field must have type Service<K>",
                ));
            };
            names.push(&field.ident);
            keys.push(key);
        }
        let name = &input.ident;
        let (ig, tg, wc) = input.generics.split_for_impl();
        Ok(quote! {
            impl #ig ::hoh_kernel::Inject for #name #tg #wc {
                fn deps() -> ::std::vec::Vec<::hoh_kernel::Dep> { vec![#(::hoh_kernel::Dep { name: <#keys as ::hoh_kernel::ServiceKey>::NAME, key: ::std::any::TypeId::of::<#keys>() }),*] }
                fn resolve(view: &::hoh_kernel::View) -> Self { Self { #(#names: view.service::<#keys>()),* } }
            }
        })
    })();
    result.unwrap_or_else(syn::Error::into_compile_error).into()
}
struct PluginArgs {
    ident: syn::Ident,
    name: syn::LitStr,
}
impl syn::parse::Parse for PluginArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let ident = input.parse()?;
        input.parse::<syn::Token![,]>()?;
        let key: syn::Ident = input.parse()?;
        if key != "name" {
            return Err(syn::Error::new(
                key.span(),
                "expected name = \"plugin-name\"",
            ));
        }
        input.parse::<syn::Token![=]>()?;
        Ok(Self {
            ident,
            name: input.parse()?,
        })
    }
}
#[proc_macro_attribute]
pub fn plugin(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as PluginArgs);
    let f = parse_macro_input!(input as syn::ItemFn);
    let result = (|| {
        if f.sig.asyncness.is_none() {
            return Err(syn::Error::new(
                f.sig.span(),
                "plugin function must be async",
            ));
        }
        let mut deps = quote! { () };
        let mut config = quote! { ::hoh_kernel::NoConfig };
        let mut call = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for arg in &f.sig.inputs {
            let syn::FnArg::Typed(arg) = arg else {
                return Err(syn::Error::new(
                    arg.span(),
                    "plugin parameter must be ctx, deps, or config",
                ));
            };
            let syn::Pat::Ident(pat) = &*arg.pat else {
                return Err(syn::Error::new(
                    arg.span(),
                    "plugin parameter must be named ctx, deps, or config",
                ));
            };
            let name = pat.ident.to_string();
            let name = name.trim_start_matches('_');
            if !seen.insert(name.to_owned()) {
                return Err(syn::Error::new(pat.span(), "duplicate plugin parameter"));
            }
            match name {
                "ctx" => call.push(quote! { ctx }),
                "deps" => {
                    let ty = &arg.ty;
                    deps = quote! { #ty };
                    call.push(quote! { deps });
                }
                "config" => {
                    let Some(ty) = wrapped(&arg.ty, "Cfg") else {
                        return Err(syn::Error::new(
                            arg.ty.span(),
                            "plugin config parameter must have type Cfg<C>",
                        ));
                    };
                    config = quote! { #ty };
                    call.push(quote! { config });
                }
                _ => {
                    return Err(syn::Error::new(
                        pat.span(),
                        "plugin parameter must be named ctx, deps, or config",
                    ));
                }
            }
        }
        if !seen.contains("ctx") {
            return Err(syn::Error::new(
                f.sig.span(),
                "plugin function requires a ctx parameter",
            ));
        }
        let ident = args.ident;
        let name = args.name;
        let fn_name = &f.sig.ident;
        Ok(quote! {
            #f
            pub struct #ident;
            impl ::hoh_kernel::Plugin for #ident {
                const NAME: &'static str = #name;
                type Config = #config; type Deps = #deps;
                async fn apply(ctx: ::hoh_kernel::Context, deps: Self::Deps, config: ::hoh_kernel::Cfg<Self::Config>) -> Result<(), ::hoh_kernel::Error> {
                    let _ = (&deps, &config); #fn_name(#(#call),*).await
                }
            }
        })
    })();
    result.unwrap_or_else(syn::Error::into_compile_error).into()
}
