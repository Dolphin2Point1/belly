use proc_macro2::{Span, TokenStream};
use quote::*;
extern crate proc_macro;
use syn::{parse_macro_input, spanned::Spanned, DeriveInput, Error, Expr, ExprPath};
use syn_rsx::{parse, Node, NodeAttribute};

fn create_single_command_stmt(expr: &ExprPath) -> TokenStream {
    let component_span = expr.span();
    if let Some(component) = expr.path.get_ident() {
        if component.to_string().chars().next().unwrap().is_uppercase() {
            quote_spanned! {component_span=>
                c.insert(#component::default());
            }
        } else {
            quote_spanned! {component_span=>
                c.insert(#component);
            }
        }
    } else {
        Error::new(component_span, "Invalid components declaration").into_compile_error()
    }
}

fn create_command_stmts(expr: &Expr) -> TokenStream {
    let with_body = match expr {
        Expr::Path(path) => create_single_command_stmt(path),
        Expr::Tuple(components) => {
            let mut components_expr = quote! {};
            for component_expr in components.elems.iter() {
                let component_span = component_expr.span();
                if let Expr::Path(component) = component_expr {
                    let component_expr = create_single_command_stmt(component);
                    components_expr = quote_spanned! {component_span=>
                        #components_expr
                        #component_expr
                    };
                } else {
                    return Error::new(component_span, "Invalid component name")
                        .into_compile_error();
                }
            }
            components_expr
        }
        _ => {
            return Error::new(expr.span(), "Invalid components declaration").into_compile_error();
        }
    };
    let expr_span = expr.span();
    quote_spanned! {expr_span=>
        __ctx.params.add(::bevy_elements_core::Variant::from_commands("with", ::std::boxed::Box::new(move |c| {
            #with_body
        })));
    }
}

fn create_attr_stmt(attr: &NodeAttribute) -> TokenStream {
    let attr_name = attr.key.to_string();
    match &attr.value {
        None => {
            return quote! {
                __ctx.params.add(::bevy_elements_core::params::Param::new(
                    #attr_name.into(),
                    ::bevy_elements_core::Variant::Empty
                ));
            };
        }
        Some(attr_value) => {
            let attr_value = attr_value.as_ref();
            let attr_span = attr_value.span();
            if attr_name == "with" {
                return create_command_stmts(attr_value);
            } else {
                return quote_spanned! {attr_span=>
                    __ctx.params.add(::bevy_elements_core::params::Param::new(
                        #attr_name.into(),
                        (#attr_value).into()
                    ));
                };
            }
        }
    }
}

fn walk_nodes<'a>(element: &'a Node, create_entity: bool) -> TokenStream {
    let mut children = quote! {};
    let mut connections = quote! {};
    let mut parent = if create_entity {
        quote! { let __parent = __world.spawn_empty().id(); }
    } else {
        quote! {}
    };
    if let Node::Element(element) = element {
        let mut parent_defined = false;
        for attr in element.attributes.iter() {
            if let Node::Block(entity) = attr {
                let entity_span = entity.value.span();
                let entity = entity.value.as_ref();
                if parent_defined {
                    return Error::new(entity_span, "Entity already provided by entity attribute")
                        .into_compile_error();
                }
                parent_defined = true;
                parent = quote! {
                    let __parent = #entity;
                };
            } else if let Node::Attribute(attr) = attr {
                let attr_name = attr.key.to_string();
                let attr_span = attr.span();
                if let Some(signal) = attr_name.strip_prefix("on:") {
                    let Some(connection) = attr.value.as_ref() else {
                        return Error::new(attr_span, format!("on:{signal} param should provide connection"))
                            .into_compile_error();
                    };
                    let connection = connection.as_ref();
                    let signal_ident = syn::Ident::new(signal, connection.span());
                    connections = quote_spanned! {attr_span=>
                        #connections
                        __builder.#signal_ident(__world, __parent, #connection);
                    }
                } else if let Some(prop) = attr_name.strip_prefix("bind:") {
                    let Some(bind) = attr.value.as_ref() else {
                        return Error::new(attr_span, format!("bind:{prop} param should provide connection"))
                            .into_compile_error();
                    };
                    let bind = bind.as_ref();
                    let stream = bind.to_token_stream().to_string();
                    if stream.trim().starts_with("to!") {
                        let bind_to = format!("bind_{}_to", prop);
                        let bind_ident = syn::Ident::new(&bind_to, bind.span());
                        connections = quote_spanned! {attr_span=>
                            #connections
                            __builder.#bind_ident(__world, __parent, #bind);
                        };
                    } else if stream.trim().starts_with("from!") {
                        let bind_from = format!("bind_{}_from", prop);
                        let bind_ident = syn::Ident::new(&bind_from, bind.span());
                        connections = quote_spanned! {attr_span=>
                            #connections
                            __builder.#bind_ident(__world, __parent, #bind);
                        };
                    }
                    // panic!("bind def: {}", bind_def.to_token_stream());
                    // let signal_ident = syn::Ident::new(signal, connection.span());
                    // connections = quote_spanned! {attr_span=>
                    //     #connections
                    //     __builder.#signal_ident(__world, __parent, #connection);
                    // }
                } else if &attr_name == "entity" {
                    if parent_defined {
                        return Error::new(attr_span, "Entity already provided by braced block")
                            .into_compile_error();
                    }
                    parent_defined = true;
                    let attr_value = attr.value.as_ref();
                    if attr_value.is_none() {
                        return Error::new(attr_span, "Attriute entity should has a value")
                            .into_compile_error();
                    }
                    let entity = attr_value.unwrap().as_ref();
                    parent = quote_spanned! { attr_span=>
                        let __parent = #entity;
                    };
                } else {
                    let attr_stmt = create_attr_stmt(attr);
                    children = quote! {
                        #children
                        #attr_stmt
                    };
                }
            }
        }
        for child in element.children.iter() {
            match child {
                Node::Element(_) => {
                    let expr = walk_nodes(child, true);
                    children = quote! {
                        #children
                        __ctx.children.push( #expr );
                    };
                }
                Node::Text(text) => {
                    let text = text.value.as_ref();
                    children = quote! {
                        #children
                        __ctx.children.push(
                            __world.spawn(::bevy::prelude::TextBundle {
                                text: ::bevy::prelude::Text::from_section(
                                    #text,
                                    ::std::default::Default::default()
                                ),
                                ..default()
                            })
                            .insert(::bevy_elements_core::Element::inline())
                            .id()
                        );
                    };
                }
                Node::Block(block) => {
                    let block = block.value.as_ref();
                    let block_span = block.span();
                    children = quote_spanned! { block_span=>
                        #children
                        let __node = __world.spawn_empty().id();
                        for __child in #block.into_content(__node, __world).iter() {
                            __ctx.children.push( __child.clone() );
                        }
                    }
                }
                _ => (),
            };
        }

        let tag = syn::Ident::new(&element.name.to_string(), element.span());
        quote! {
            {
                #parent
                let mut __ctx = ::bevy_elements_core::eml::build::ElementContextData::new(__parent);

                #children
                let __builder = ::bevy_elements_core::Widgets::#tag();
                __builder.get_builder().build(__world, __ctx);
                #connections
                __parent
            }
        }
    } else {
        quote! {}
    }
}

#[proc_macro]
pub fn eml(tree: proc_macro::TokenStream) -> proc_macro::TokenStream {
    match parse(tree.into()) {
        Err(err) => err.to_compile_error().into(),
        Ok(nodes) => {
            let body = walk_nodes(&nodes[0], false);
            // nodes[0]
            let wraped = quote! {
                ::bevy_elements_core::ElementsBuilder::new(
                    move |
                        __world: &mut ::bevy::prelude::World,
                        __parent: ::bevy::prelude::Entity
                    | {
                        #body;
                    }
            )};
            proc_macro::TokenStream::from(wraped)
        }
    }
}

fn err(span: Span, message: &str) -> proc_macro::TokenStream {
    proc_macro::TokenStream::from(syn::Error::new(span, message).to_compile_error())
}

#[proc_macro_derive(Widget, attributes(alias, param, signal))]
pub fn widget_macro_derive(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let span = ast.span();

    let component = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
    let component_str = format!("{component}");
    let mut alias_expr = quote! { #component_str };
    let mod_descriptor = format_ident!("{}_widget_descriptor", component_str.to_lowercase());
    let extension_ident = format_ident!("{component}WidgetExtension");
    let mut construct_body = quote! {};

    // TODO: use `doclines` to generate XSD docs like extension docs
    let (_doclines, docs) = parse_docs(&ast.attrs);
    let mut bind_body = quote! {
        let this = ctx.entity();
    };
    let mut extension_body = quote! {
        #[allow(non_snake_case)]
        fn #component() -> #mod_descriptor::Descriptor {
            #mod_descriptor::Descriptor
        }
    };

    let syn::Data::Struct(data) = &ast.data else {
        return err(span, "Widget could be derived only for structs");
    };

    for field in data.fields.iter() {
        let Some(field_ident) = field.ident.as_ref() else {
            return err(span, "Tuple Structs not yet supported by Widget derive.")
        };
        let field_str = format!("{field_ident}");

        if let syn::Type::Path(path) = &field.ty {
            let type_repr = path.clone().into_token_stream().to_string();
            if &type_repr == "Entity" {
                if &field_str == "ctx" {
                    return err(
                        field.span(),
                        "Using `ctx` as field name may lead to Widget's unexpected behaviour.",
                    );
                }
                if &field_str == "this" {
                    return err(
                        field.span(),
                        "Using `this` as field name may lead to Widget's unexpected behaviour.",
                    );
                }
                construct_body = quote! {
                    #construct_body
                    #field_ident: world.spawn_empty().id(),
                };
                bind_body = quote! {
                    #bind_body
                    let #field_ident = self.#field_ident;
                };
                continue;
            }
        }
        construct_body = quote! {
            #construct_body
            #field_ident: ::std::default::Default::default(),
        }
    }

    for field in data.fields.iter() {
        let field_ident = &field.ident;
        for attr in field.attrs.iter() {
            if !attr.path.is_ident("param") {
                continue;
            };
            let field_type = &field.ty;
            bind_body = quote! {
                #bind_body
                if let ::std::option::Option::Some(value) =
                    ::bevy_elements_core::bindattr!(ctx, #field_ident:#field_type => Self.#field_ident)
                {
                    self.#field_ident = value;
                }
            };
            if !attr.tokens.is_empty() {
                let bind_target = attr.parse_args::<TokenStream>().unwrap();
                bind_body = quote! {
                    #bind_body
                    ctx.commands().add(
                        ::bevy_elements_core::bind!(this, #component.#field_ident #bind_target)
                    );
                }
            }
        }
    }

    for attr in ast.attrs.iter() {
        if attr.path.is_ident("alias") {
            let Ok(alias) = attr.parse_args::<syn::Ident>() else {
                return err(attr.span(), "Alias should be defined using tokens: `#[alias(alias_name)]");
            };
            let alias_str = format!("{alias}");
            alias_expr = quote! { #alias_expr, #alias_str };
            extension_body = quote! {
                #extension_body
                #docs
                fn #alias() -> #mod_descriptor::Descriptor {
                    #mod_descriptor::Descriptor
                }
            }
        }
    }

    let connect_signals = match parse_signals(&ast.attrs) {
        Ok(tokens) => tokens,
        Err(e) => return proc_macro::TokenStream::from(e.to_compile_error()),
    };

    let bind_descriptors = match parse_binds(&ast) {
        Ok(tokens) => tokens,
        Err(e) => return proc_macro::TokenStream::from(e.to_compile_error()),
    };
    // panic!("binds: {}", bind_descriptors.to_string());

    proc_macro::TokenStream::from(quote! {
        mod #mod_descriptor {
            use super::*;
            pub struct Descriptor;
            impl Descriptor {
                pub fn get_instance() -> &'static Descriptor {
                    &&Descriptor
                }

                pub fn get_builder(&self) -> ::bevy_elements_core::ElementBuilder {
                    #component::as_builder()
                }
                #connect_signals

                #bind_descriptors
            }
        }
        impl #impl_generics ::bevy_elements_core::Widget for #component #ty_generics #where_clause {
            fn names() -> &'static [&'static str] {
                &[#alias_expr]
            }
            fn construct_component(world: &mut ::bevy::prelude::World) -> ::std::option::Option<Self> {
                ::std::option::Option::Some(#component {
                    #construct_body
                })
            }
            #[allow(unused_variables)]
            fn bind_component(&mut self, ctx: &mut ::bevy_elements_core::ElementContext) {
                #bind_body
            }
        }

        pub trait #extension_ident {
            type Descriptor;
            fn descriptor() -> Self::Descriptor;
            #extension_body
        }

        impl #extension_ident for ::bevy_elements_core::Widgets {
            type Descriptor = #mod_descriptor::Descriptor;
            fn descriptor() -> Self::Descriptor {
                #mod_descriptor::Descriptor
            }
        }
    })
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        None => String::new(),
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
    }
}

fn parse_binds(ast: &syn::DeriveInput) -> syn::Result<TokenStream> {
    let component = &ast.ident;
    let span = ast.span();
    let mut binds = quote! {};
    let syn::Data::Struct(data) = &ast.data else {
        return Err(syn::Error::new(span, "Widget could be derived only for structs"));
    };
    for field in data.fields.iter() {
        let Some(field_ident) = field.ident.as_ref() else {
            return  Err(syn::Error::new(span, "Tuple Structs not yet supported by Widget derive."));
        };
        if field
            .attrs
            .iter()
            .filter(|a| a.path.is_ident("param"))
            .next()
            .is_none()
        {
            continue;
        }
        let field_name = format!("{}", field_ident);

        // let field_str = format!("{field_ident}");
        // for attr in field.attrs.iter() {
        //     if !attr.path.is_ident("param") {
        //         continue;
        //     };
        // }

        let field_type = &field.ty;
        let bind_to_ident = format_ident!("bind_{}_to", field_ident);
        let bind_from_ident = format_ident!("bind_{}_from", field_ident);
        binds = quote! {
            #binds

            pub fn #bind_to_ident <W: ::bevy::prelude::Component, T: ::bevy_elements_core::relations::bound::BindableTarget>(
                &self,
                world: &mut ::bevy::prelude::World,
                source: ::bevy::prelude::Entity,
                bind: ::bevy_elements_core::relations::bound::Bind<
                    #component, W, #field_type, T
                >
            ) {
                let ::bevy_elements_core::relations::bound::Bind::To(to) = bind else { return };
                to.from(::bevy_elements_core::relations::bound::BoundFrom {
                    source,
                    reader: |c: &#component| c.#field_ident.clone(),
                    source_id: tag!(::bevy_elements_core::relations::bound::format_source_id::<#component>(#field_name)),
                }).write(world);
            }

            pub fn #bind_from_ident <R: ::bevy::prelude::Component, S: ::bevy_elements_core::relations::bound::BindableSource>(
                &self,
                world: &mut ::bevy::prelude::World,
                target: Entity,
                bind: ::bevy_elements_core::relations::bound::Bind<
                    R, #component, S, #field_type
                >
            ) {
                let ::bevy_elements_core::relations::bound::Bind::From(from, transformer) = bind else { return };
                from.to(::bevy_elements_core::relations::bound::BoundTo {
                    target,
                    target_id: tag!(::bevy_elements_core::relations::bound::format_source_id::<#component>(#field_name)),
                    transformer: transformer,
                    reader: |c: &#component| &c.#field_ident,
                    writer: |c: &mut #component, v| c.#field_ident = v
                }).write(world);

            }
        };
    }
    Ok(binds)
}

fn parse_docs(attrs: &Vec<syn::Attribute>) -> (Vec<String>, TokenStream) {
    let mut docs = quote! {};
    let mut doclines = vec![];

    for attr in attrs.iter().filter(|a| a.path.is_ident("doc")) {
        let docline = attr.tokens.to_string();
        if docline.starts_with("= ") {
            doclines.push(docline[2..].to_string());
        }
        docs = quote! {
            #docs
            #attr
        }
    }
    (doclines, docs)
}

fn parse_signals(attrs: &Vec<syn::Attribute>) -> syn::Result<TokenStream> {
    let mut connect_body = quote! {};
    for attr in attrs.iter() {
        if attr.path.is_ident("signal") {
            let span = attr.span();
            // let signal_decl = attr.tokens.clone();
            let Ok(meta) = attr.parse_meta() else {
                return Err(syn::Error::new(span,  "Invalid syntax fo #[signal(name, Event, filter)] attribute."));
            };
            let syn::Meta::List(signal_cfg) = meta else {
                return Err(syn::Error::new(span, "Invalid syntax fo #[signal(name, Event, filter)] attribute."));    
            };
            let signal_cfg: Vec<_> = signal_cfg.nested.iter().collect();
            if signal_cfg.len() != 3 {
                return Err(syn::Error::new(
                    span,
                    "Invalid syntax fo #[signal(name, Event, filter)] attribute.",
                ));
            }
            let syn::NestedMeta::Meta(name) = signal_cfg[0] else {
                let span = signal_cfg[0].span();
                return Err(syn::Error::new(span, "Expected ident as first argument to #[signal(name, Event, filter)] attribute."));
            };
            let Some(name) = name.path().get_ident() else {
                let span = name.span();
                return Err(syn::Error::new(span, "Expected ident as first argument to #[signal(name, Event, filter)] attribute."));
            };
            let syn::NestedMeta::Meta(event) = signal_cfg[1] else {
                let span = signal_cfg[1].span();
                return Err(syn::Error::new(span, "Expected type path as second argument to #[signal(name, Event, filter)] attribute."));
            };
            let syn::NestedMeta::Meta(filter) = signal_cfg[2] else {
                let span = signal_cfg[2].span();
                return Err(syn::Error::new(span, "Expected ident as third argument to #[signal(name, Event, filter)] attribute."));
            };
            let Some(filter) = filter.path().get_ident() else {
                let span = filter.span();
                return Err(syn::Error::new(span, "Expected ident as third argument to #[signal(name, Event, filter)] attribute."));
            };
            let event = event.path();
            connect_body = quote! {
                #connect_body
                pub fn #name<C: ::bevy::prelude::Component>(
                    &self,
                    world: &mut ::bevy::prelude::World,
                    source: ::bevy::prelude::Entity,
                    target: ::bevy_elements_core::ConnectionTo<C, #event>
                ) {
                    target
                        .filter(|e| e.#filter())
                        .from(source)
                        .write(world)
                }
            }
        }
    }
    Ok(connect_body)
}

fn parse_extends(ident: &syn::Ident, attrs: &Vec<syn::Attribute>) -> syn::Result<TokenStream> {
    let Some(attr) = attrs.iter().filter(|a| a.path.is_ident("extends")).next() else {
        return Ok(quote! {})
    };
    let Ok(extends) = attr.parse_args::<syn::Ident>() else {
        return Err(syn::Error::new(attr.span(), "#[extends] should be defined using token: `#[extends(button)]"));
    };
    let this_str = ident.to_string();
    let extends = extends.to_string();
    let this_mod = format_ident!("{}_widget_descriptor", this_str.to_lowercase());
    let extends = format_ident!("{}WidgetExtension", capitalize(&extends));
    let derive = quote! {
        impl ::std::ops::Deref for #this_mod::Descriptor {
            type Target = <::bevy_elements_core::Widgets as #extends>::Descriptor;
            fn deref(&self) -> &<::bevy_elements_core::Widgets as #extends>::Descriptor {
                let instance = <::bevy_elements_core::Widgets as #extends>::Descriptor::get_instance();
                instance
            }
        }
    };
    Ok(derive)
}

fn parse_styles(ident: &syn::Ident, attrs: &Vec<syn::Attribute>) -> syn::Result<TokenStream> {
    let mut styles = "".to_string();
    let element = ident.to_string();
    for attr in attrs.iter().filter(|a| a.path.is_ident("style")) {
        let span = attr.span();
        let Ok(meta) = attr.parse_meta() else {
            return Err(syn::Error::new(span,  "Invalid syntax fo #[signal(name, Event, filter)] attribute."));
        };
        let syn::Meta::List(style) = meta else {
            return Err(syn::Error::new(span, "Invalid syntax fo #[signal(name, Event, filter)] attribute."));    
        };
        let style: Vec<_> = style.nested.iter().collect();
        if style.len() == 1 {
            let syn::NestedMeta::Lit(syn::Lit::Str(prop)) = style[0] else {
                let span = style[0].span();
                return Err(syn::Error::new(span, "Non-literal token ident as first argument to #[widget(\"prop: value\")] attribute."));
            };
            let style = prop.value();
            styles += &format!("{element}: {{ {style} }}\n")
        } else {
            let syn::NestedMeta::Lit(syn::Lit::Str(selector)) = style[0] else {
                let span = style[0].span();
                return Err(syn::Error::new(span, "Non-literal token in #[widget(\"selector\", \"prop: value\")] attribute."));
            };
            let selector = selector.value();
            let mut props = "".to_string();
            for prop in style.iter().skip(1) {
                let syn::NestedMeta::Lit(syn::Lit::Str(prop)) = prop else {
                    let span = style[0].span();
                    return Err(syn::Error::new(span, "Non-literal token in #[widget(\"selector\", \"prop: value\")] attribute."));
                };
                props += &prop.value();
                props += "; "
            }
            styles += &format!("{selector} {{ {props} }}");
        }

        // let Ok(style) = syn::punctuated::Punctuated::<syn::LitStr, syn::Token![,]>::parse_terminated.parse2(attrs) else {
        //     return Err(syn::Error::new(span, "#[style] macro attributes should string literals: #[widget(\"font: bold\")]"));
        // };
        // if style.len() == 1 {
        //     let style = style[0].value();
        //     styles += &format!("{element}: {{ {style} }}\n")
        // } else {
        //     let selector = style[0].value();
        //     let mut props = "".to_string();
        //     for prop in style.iter().skip(1) {
        //         props += &prop.value();
        //         props += "; ";
        //     }
        //     styles += &format!("{selector} {{ {props} }}\n");
        // }
    }

    if styles.len() > 0 {
        // let styles = format!("{element}: {{ {styles} }}");
        return Ok(quote! {
            fn styles() -> &'static str {
                #styles
            }
        });
    } else {
        return Ok(quote! {
            fn styles() -> &'static str {
                ""
            }
        });
    }
}

#[proc_macro_attribute]
pub fn widget(
    _args: proc_macro::TokenStream,
    input: proc_macro::TokenStream,
) -> proc_macro::TokenStream {
    let ast = parse_macro_input!(input as syn::ItemFn);
    let fn_ident = ast.sig.ident;
    let fn_args = ast.sig.inputs;
    let fn_body = ast.block;
    let alias = fn_ident.to_string();
    let mod_descriptor = format_ident!("{}_widget_descriptor", &alias);
    let extension = format_ident!("{}WidgetExtension", capitalize(&alias));
    let (_doclines, docs) = parse_docs(&ast.attrs);

    let connect_signals = match parse_signals(&ast.attrs) {
        Ok(tokens) => tokens,
        Err(e) => return proc_macro::TokenStream::from(e.to_compile_error()),
    };
    let extends_decl = match parse_extends(&fn_ident, &ast.attrs) {
        Ok(tokens) => tokens,
        Err(e) => return proc_macro::TokenStream::from(e.to_compile_error()),
    };
    let styles_decl = match parse_styles(&fn_ident, &ast.attrs) {
        Ok(tokens) => tokens,
        Err(e) => return proc_macro::TokenStream::from(e.to_compile_error()),
    };

    proc_macro::TokenStream::from(quote! {

        mod #mod_descriptor {
        use super::*;

        #[derive(Component)]
        #[allow(non_camel_case_types)]
        pub struct #fn_ident;

        pub struct Descriptor;
        impl Descriptor {
            pub fn get_instance() -> &'static Descriptor {
                &&Descriptor
            }

            pub fn get_builder(&self) -> ::bevy_elements_core::ElementBuilder {
                #fn_ident::as_builder()
            }
            #styles_decl
            #connect_signals
        }

        impl ::bevy_elements_core::Widget for #fn_ident {
            fn names() -> &'static [&'static str] {
                &[#alias]
            }
        }

        impl ::bevy_elements_core::WidgetBuilder for #fn_ident {
            #styles_decl
            fn construct(#fn_args) {
                #fn_body
            }
        }

        pub trait #extension {
            type Descriptor;
            #docs
            fn #fn_ident() -> Descriptor {
                Descriptor
            }

            fn descriptor() -> Self::Descriptor;
        }

        impl #extension for ::bevy_elements_core::Widgets {
            type Descriptor = Descriptor;
            fn descriptor() -> Self::Descriptor {
                Descriptor
            }
        }
        }
        pub use #mod_descriptor::#extension;
        #[allow(non_camel_case_types)]
        pub (crate) type #fn_ident = #mod_descriptor::#fn_ident;

        #extends_decl



    })
}
