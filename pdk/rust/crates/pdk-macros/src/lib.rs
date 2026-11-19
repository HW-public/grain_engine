//! `#[plugin_fn]`：把插件作者的 `async fn latest(page: u32) -> Result<Value, Error>`
//! 转成 ABI 导出的 `extern "C" fn latest(page: i32) -> i32`（注册 task 并返回 handle）。
//!
//! 支持以下参数映射：
//!   `u32`/`i32`   ← i32（原样/转型）
//!   `u64`/`i64`   ← i64
//!   `String`      ← kw_ptr:i32（宿主写入的 NUL 结尾 UTF-8 串）
//! 返回类型必须是 `Result<T, E>`（`T: serde::Serialize`，`E: Display`）。

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, Pat};

#[proc_macro_attribute]
pub fn plugin_fn(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func: ItemFn = match syn::parse(item) {
        Ok(f) => f,
        Err(e) => return e.to_compile_error().into(),
    };

    if func.sig.asyncness.is_none() {
        return syn::Error::new_spanned(func.sig.fn_token, "plugin_fn requires an async fn")
            .to_compile_error()
            .into();
    }
    if !func.sig.generics.params.is_empty() {
        return syn::Error::new_spanned(&func.sig.generics, "plugin_fn does not support generics")
            .to_compile_error()
            .into();
    }

    let outer_name = func.sig.ident.clone();
    let inner_name = format_ident!("__grain_impl_{}", outer_name);
    let vis = &func.vis;
    let output = &func.sig.output;
    let block = &func.block;

    let mut export_params: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut call_args: Vec<proc_macro2::TokenStream> = Vec::new();
    let mut inner_inputs: Vec<FnArg> = Vec::new();

    for input in &func.sig.inputs {
        let FnArg::Typed(pt) = input else {
            return syn::Error::new_spanned(input, "plugin_fn does not support self")
                .to_compile_error()
                .into();
        };
        let Pat::Ident(pi) = &*pt.pat else {
            return syn::Error::new_spanned(&pt.pat, "plugin_fn args must be simple identifiers")
                .to_compile_error()
                .into();
        };
        let ident = &pi.ident;
        let ty = &pt.ty;
        let ty_str = quote!(#ty).to_string().replace(' ', "");
        inner_inputs.push(input.clone());

        match ty_str.as_str() {
            "u32" => {
                export_params.push(quote!(#ident: i32));
                call_args.push(quote!(#ident as u32));
            }
            "i32" => {
                export_params.push(quote!(#ident: i32));
                call_args.push(quote!(#ident));
            }
            "u64" | "i64" => {
                export_params.push(quote!(#ident: i64));
                call_args.push(quote!(#ident as #ty));
            }
            "String" => {
                let ptr = format_ident!("{}_ptr", ident);
                export_params.push(quote!(#ptr: i32));
                call_args.push(quote!(unsafe { ::grain_pdk::read_cstr(#ptr) }));
            }
            other => {
                return syn::Error::new_spanned(
                    &pt.ty,
                    format!("unsupported plugin_fn arg type `{other}` (supported: u32, i32, u64, i64, String)"),
                )
                .to_compile_error()
                .into();
            }
        }
    }

    let expanded = quote! {
        #vis async fn #inner_name(#(#inner_inputs),*) #output #block

        #[no_mangle]
        pub extern "C" fn #outer_name(#(#export_params),*) -> i32 {
            ::grain_pdk::spawn(async move {
                match #inner_name(#(#call_args),*).await {
                    ::std::result::Result::Ok(v) => {
                        ::std::result::Result::Ok(::grain_pdk::to_json_string(&v))
                    }
                    ::std::result::Result::Err(e) => {
                        ::std::result::Result::Err(::grain_pdk::Error::msg(
                            ::std::string::ToString::to_string(&e),
                        ))
                    }
                }
            })
        }
    };

    expanded.into()
}
