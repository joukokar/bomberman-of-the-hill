use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{ItemTrait, Pat, ReturnType, TraitItem, parse_macro_input};

pub fn implementation(input: TokenStream) -> TokenStream {
    let trait_block = parse_macro_input!(input as ItemTrait);

    let methods: Vec<_> = trait_block.items.iter().filter_map(|i| {
        if let TraitItem::Method(m) = i { Some(m) } else { None }
    }).collect();

    let wrappers = methods.iter().map(|m| build_wasm_wrapper(m));

    let expanded = quote! {
        #trait_block

        #[derive(::serde::Serialize, ::serde::Deserialize)]
        struct __WasmOutputLocator { address: i32, size: u32 }

        #(#wrappers)*
    };

    TokenStream::from(expanded)
}

fn build_wasm_wrapper(method: &syn::TraitItemMethod) -> quote::__private::TokenStream {
    let wrapper_identifier = format_ident!("wasm_{}", method.sig.ident.clone());
    let shim_identifier = format!("__wasm_{}", method.sig.ident.clone());

    // We can only work with non-self arguments represented by an identifier
    let valid_inputs: Vec<_> = method.sig.inputs
        .iter()
        .filter(|i| match i {
            syn::FnArg::Typed(t) if matches!(&*t.pat, Pat::Ident(_))=> true,
            _ => false,
        })
        .collect();

    let input_patterns: Vec<_> = valid_inputs.iter().filter_map(|i| {
        if let syn::FnArg::Typed(t) = i {
            if let Pat::Ident(id) = &*t.pat {
                Some(id)
            } else {
                None
            }
        } else {
            None
        }
    }).collect();

    let shim_input_addresses: Vec<_> = input_patterns.iter().map(|p| format_ident!("{}_address", p.ident)).collect();
    let shim_input_lengths: Vec<_> = input_patterns.iter().map(|p| format_ident!("{}_length", p.ident)).collect();
    let shim_input_types = (0..valid_inputs.len() * 2).map(|_| format_ident!("i32"));

    let shim_output_type = if matches!(method.sig.output, ReturnType::Type(..)) {
        format_ident!("i32")
    } else {
        format_ident!("()")
    };

    let input_processing = quote! {
        let memory = instance.get_memory(store.as_context_mut(), "memory")
            .ok_or(anyhow::anyhow!("Wasm memory block not found"))?;
        let get_input_buffer_address = instance.get_typed_func::<(), i32, _>(
            store.as_context_mut(), "__wasm_get_input_buffer_address"
        )?;
        let mut input_buffer_address = get_input_buffer_address.call(store.as_context_mut(), ())?;

        #(
            let #input_patterns = bincode::serialize(&#input_patterns)?;
            let #shim_input_addresses = input_buffer_address as usize;
            let #shim_input_lengths = #input_patterns.as_slice().len();
            memory.write(store.as_context_mut(), #shim_input_addresses, #input_patterns.as_slice())?;
            input_buffer_address += #shim_input_lengths as i32;
        )*

        let method = instance.get_typed_func::<(#(#shim_input_types),*), #shim_output_type, _>(store.as_context_mut(), #shim_identifier)?;
    };

    let expanded = if let ReturnType::Type(_, ref output) = method.sig.output {
        quote! {
            pub fn #wrapper_identifier(
                store: &mut ::wasmtime::Store<()>,
                instance: & ::wasmtime::Instance,
                #(#valid_inputs),*
            ) -> ::anyhow::Result<#output> {
                #input_processing
                let locator_address = method.call(store.as_context_mut(),(#(#shim_input_addresses as _, #shim_input_lengths as _),*))?;
                let mut buffer = [0u8; core::mem::size_of::<__WasmOutputLocator>()];
                memory.read(store.as_context_mut(), locator_address as usize, &mut buffer)?;
                let locator: __WasmOutputLocator = bincode::deserialize(&buffer)?;
                let mut dynamic_buffer = vec![0u8; locator.size as usize];
                memory.read(store.as_context_mut(), locator.address as usize, dynamic_buffer.as_mut_slice())?;
                Ok(bincode::deserialize(dynamic_buffer.as_slice())?)
            }
        }
    } else {
        quote! {
            pub fn #wrapper_identifier(
                store: &mut ::wasmtime::Store<()>,
                instance: & ::wasmtime::Instance,
                #(#valid_inputs),*
            ) {
                #input_processing
                method.call(store.as_context_mut(),(#(#shim_input_addresses as _, #shim_input_lengths as _),*))?;
                Ok(())
            }
        }
    };

    expanded
}
