// Copyright 2018 Guillaume Pinot (@TeXitoi) <texitoi@texitoi.eu>,
// Kevin Knapp (@kbknapp) <kbknapp@gmail.com>, and
// Ana Hobden (@hoverbear) <operator@hoverbear.org>
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.
//
// This work was derived from Structopt (https://github.com/TeXitoi/structopt)
// commit#ea76fa1b1b273e65e3b0b1046643715b49bec51f which is licensed under the
// MIT/Apache 2.0 license.

use std::env;

use heck::{ToKebabCase, ToLowerCamelCase, ToShoutySnakeCase, ToSnakeCase, ToUpperCamelCase};
use proc_macro2::{self, Span, TokenStream};
use proc_macro_error::abort;
use quote::{quote, quote_spanned, ToTokens};
use syn::DeriveInput;
use syn::{
    self, ext::IdentExt, spanned::Spanned, Attribute, Field, Ident, LitStr, MetaNameValue, Type,
    Variant,
};

use crate::attr::*;
use crate::utils::{inner_type, is_simple_ty, process_doc_comment, Sp, Ty};

/// Default casing style for generated arguments.
pub const DEFAULT_CASING: CasingStyle = CasingStyle::Kebab;

/// Default casing style for environment variables
pub const DEFAULT_ENV_CASING: CasingStyle = CasingStyle::ScreamingSnake;

#[derive(Clone)]
pub struct Item {
    name: Name,
    casing: Sp<CasingStyle>,
    env_casing: Sp<CasingStyle>,
    ty: Option<Type>,
    doc_comment: Vec<Method>,
    methods: Vec<Method>,
    deprecations: Vec<Deprecation>,
    value_parser: Option<ValueParser>,
    action: Option<Action>,
    verbatim_doc_comment: bool,
    next_display_order: Option<Method>,
    next_help_heading: Option<Method>,
    is_enum: bool,
    is_positional: bool,
    kind: Sp<Kind>,
}

impl Item {
    pub fn from_args_struct(input: &DeriveInput, name: Name) -> Self {
        let span = Span::call_site();
        let attrs = &input.attrs;
        let argument_casing = Sp::call_site(DEFAULT_CASING);
        let env_casing = Sp::call_site(DEFAULT_ENV_CASING);
        let kind = Sp::new(Kind::Command(Sp::new(Ty::Other, span)), span);
        Self::from_struct(attrs, name, argument_casing, env_casing, kind)
    }

    pub fn from_subcommand_enum(input: &DeriveInput, name: Name) -> Self {
        let span = Span::call_site();
        let attrs = &input.attrs;
        let argument_casing = Sp::call_site(DEFAULT_CASING);
        let env_casing = Sp::call_site(DEFAULT_ENV_CASING);
        let kind = Sp::new(Kind::Command(Sp::new(Ty::Other, span)), span);
        Self::from_struct(attrs, name, argument_casing, env_casing, kind)
    }

    pub fn from_value_enum(input: &DeriveInput, name: Name) -> Self {
        let span = Span::call_site();
        let attrs = &input.attrs;
        let argument_casing = Sp::call_site(DEFAULT_CASING);
        let env_casing = Sp::call_site(DEFAULT_ENV_CASING);
        let kind = Sp::new(Kind::Value(Sp::new(Ty::Other, span)), span);
        Self::from_struct(attrs, name, argument_casing, env_casing, kind)
    }

    fn from_struct(
        attrs: &[Attribute],
        name: Name,
        argument_casing: Sp<CasingStyle>,
        env_casing: Sp<CasingStyle>,
        kind: Sp<Kind>,
    ) -> Self {
        let mut res = Self::new(name, None, argument_casing, env_casing, kind);
        res.push_attrs(attrs);
        res.push_doc_comment(attrs, "about");

        if let Some(value_parser) = res.value_parser.as_ref() {
            abort!(
                value_parser.span(),
                "`value_parser` attribute is only allowed on fields"
            );
        }
        if let Some(action) = res.action.as_ref() {
            abort!(
                action.span(),
                "`action` attribute is only allowed on fields"
            );
        }

        res
    }

    pub fn from_subcommand_variant(
        variant: &Variant,
        struct_casing: Sp<CasingStyle>,
        env_casing: Sp<CasingStyle>,
    ) -> Self {
        let name = variant.ident.clone();
        let span = variant.span();
        let kind = Sp::new(Kind::Command(Sp::new(Ty::Other, span)), span);
        let mut res = Self::new(Name::Derived(name), None, struct_casing, env_casing, kind);
        res.push_attrs(&variant.attrs);
        res.push_doc_comment(&variant.attrs, "about");

        match &*res.kind {
            Kind::Flatten => {
                if let Some(value_parser) = res.value_parser.as_ref() {
                    abort!(
                        value_parser.span(),
                        "`value_parser` attribute is not allowed for flattened entry"
                    );
                }
                if let Some(action) = res.action.as_ref() {
                    abort!(
                        action.span(),
                        "`action` attribute is not allowed for flattened entry"
                    );
                }
                if res.has_explicit_methods() {
                    abort!(
                        res.kind.span(),
                        "methods are not allowed for flattened entry"
                    );
                }

                // ignore doc comments
                res.doc_comment = vec![];
            }

            Kind::Subcommand(_) => {
                if let Some(value_parser) = res.value_parser.as_ref() {
                    abort!(
                        value_parser.span(),
                        "`value_parser` attribute is not allowed for subcommand"
                    );
                }
                if let Some(action) = res.action.as_ref() {
                    abort!(
                        action.span(),
                        "`action` attribute is not allowed for subcommand"
                    );
                }

                use syn::Fields::*;
                use syn::FieldsUnnamed;
                let field_ty = match variant.fields {
                    Named(_) => {
                        abort!(variant.span(), "structs are not allowed for subcommand");
                    }
                    Unit => abort!(variant.span(), "unit-type is not allowed for subcommand"),
                    Unnamed(FieldsUnnamed { ref unnamed, .. }) if unnamed.len() == 1 => {
                        &unnamed[0].ty
                    }
                    Unnamed(..) => {
                        abort!(
                            variant,
                            "non single-typed tuple is not allowed for subcommand"
                        )
                    }
                };
                let ty = Ty::from_syn_ty(field_ty);
                match *ty {
                    Ty::OptionOption => {
                        abort!(
                            field_ty,
                            "Option<Option<T>> type is not allowed for subcommand"
                        );
                    }
                    Ty::OptionVec => {
                        abort!(
                            field_ty,
                            "Option<Vec<T>> type is not allowed for subcommand"
                        );
                    }
                    _ => (),
                }

                res.kind = Sp::new(Kind::Subcommand(ty), res.kind.span());
            }

            Kind::ExternalSubcommand
            | Kind::FromGlobal(_)
            | Kind::Skip(_, _)
            | Kind::Command(_)
            | Kind::Value(_)
            | Kind::Arg(_) => (),
        }

        res
    }

    pub fn from_value_enum_variant(
        variant: &Variant,
        argument_casing: Sp<CasingStyle>,
        env_casing: Sp<CasingStyle>,
    ) -> Self {
        let span = variant.span();
        let kind = Sp::new(Kind::Value(Sp::new(Ty::Other, span)), span);
        let mut res = Self::new(
            Name::Derived(variant.ident.clone()),
            None,
            argument_casing,
            env_casing,
            kind,
        );
        res.push_attrs(&variant.attrs);
        res.push_doc_comment(&variant.attrs, "help");

        if let Some(value_parser) = res.value_parser.as_ref() {
            abort!(
                value_parser.span(),
                "`value_parser` attribute is only allowed on fields"
            );
        }
        if let Some(action) = res.action.as_ref() {
            abort!(
                action.span(),
                "`action` attribute is only allowed on fields"
            );
        }

        res
    }

    pub fn from_args_field(
        field: &Field,
        struct_casing: Sp<CasingStyle>,
        env_casing: Sp<CasingStyle>,
    ) -> Self {
        let name = field.ident.clone().unwrap();
        let span = field.span();
        let kind = Sp::new(Kind::Arg(Sp::new(Ty::Other, span)), span);
        let mut res = Self::new(
            Name::Derived(name),
            Some(field.ty.clone()),
            struct_casing,
            env_casing,
            kind,
        );
        res.push_attrs(&field.attrs);
        res.push_doc_comment(&field.attrs, "help");

        match &*res.kind {
            Kind::Flatten => {
                if let Some(value_parser) = res.value_parser.as_ref() {
                    abort!(
                        value_parser.span(),
                        "`value_parser` attribute is not allowed for flattened entry"
                    );
                }
                if let Some(action) = res.action.as_ref() {
                    abort!(
                        action.span(),
                        "`action` attribute is not allowed for flattened entry"
                    );
                }
                if res.has_explicit_methods() {
                    abort!(
                        res.kind.span(),
                        "methods are not allowed for flattened entry"
                    );
                }

                // ignore doc comments
                res.doc_comment = vec![];
            }

            Kind::Subcommand(_) => {
                if let Some(value_parser) = res.value_parser.as_ref() {
                    abort!(
                        value_parser.span(),
                        "`value_parser` attribute is not allowed for subcommand"
                    );
                }
                if let Some(action) = res.action.as_ref() {
                    abort!(
                        action.span(),
                        "`action` attribute is not allowed for subcommand"
                    );
                }
                if res.has_explicit_methods() {
                    abort!(
                        res.kind.span(),
                        "methods in attributes are not allowed for subcommand"
                    );
                }

                let ty = Ty::from_syn_ty(&field.ty);
                match *ty {
                    Ty::OptionOption => {
                        abort!(
                            field.ty,
                            "Option<Option<T>> type is not allowed for subcommand"
                        );
                    }
                    Ty::OptionVec => {
                        abort!(
                            field.ty,
                            "Option<Vec<T>> type is not allowed for subcommand"
                        );
                    }
                    _ => (),
                }

                res.kind = Sp::new(Kind::Subcommand(ty), res.kind.span());
            }
            Kind::Skip(_, _) => {
                if res.has_explicit_methods() {
                    abort!(
                        res.kind.span(),
                        "methods are not allowed for skipped fields"
                    );
                }
            }
            Kind::FromGlobal(orig_ty) => {
                let ty = Ty::from_syn_ty(&field.ty);
                res.kind = Sp::new(Kind::FromGlobal(ty), orig_ty.span());
            }
            Kind::Arg(_) => {
                let ty = Ty::from_syn_ty(&field.ty);

                match *ty {
                    Ty::Option => {
                        if let Some(m) = res.find_default_method() {
                            abort!(m.name, "default_value is meaningless for Option")
                        }
                    }
                    Ty::OptionOption => {
                        if res.is_positional() {
                            abort!(
                                field.ty,
                                "Option<Option<T>> type is meaningless for positional argument"
                            )
                        }
                    }
                    Ty::OptionVec => {
                        if res.is_positional() {
                            abort!(
                                field.ty,
                                "Option<Vec<T>> type is meaningless for positional argument"
                            )
                        }
                    }

                    _ => (),
                }
                res.kind = Sp::new(
                    Kind::Arg(ty),
                    field
                        .ident
                        .as_ref()
                        .map(|i| i.span())
                        .unwrap_or_else(|| field.ty.span()),
                );
            }

            Kind::Command(_) | Kind::Value(_) | Kind::ExternalSubcommand => {}
        }

        res
    }

    fn new(
        name: Name,
        ty: Option<Type>,
        casing: Sp<CasingStyle>,
        env_casing: Sp<CasingStyle>,
        kind: Sp<Kind>,
    ) -> Self {
        Self {
            name,
            ty,
            casing,
            env_casing,
            doc_comment: vec![],
            methods: vec![],
            deprecations: vec![],
            value_parser: None,
            action: None,
            verbatim_doc_comment: false,
            next_display_order: None,
            next_help_heading: None,
            is_enum: false,
            is_positional: true,
            kind,
        }
    }

    fn push_method(&mut self, name: Ident, arg: impl ToTokens) {
        if name == "name" || name == "id" {
            self.name = Name::Assigned(quote!(#arg));
        } else if name == "value_parser" {
            self.value_parser = Some(ValueParser::Explicit(Method::new(name, quote!(#arg))));
        } else if name == "action" {
            self.action = Some(Action::Explicit(Method::new(name, quote!(#arg))));
        } else {
            if name == "short" || name == "long" {
                self.is_positional = false;
            }
            self.methods.push(Method::new(name, quote!(#arg)));
        }
    }

    fn push_attrs(&mut self, attrs: &[Attribute]) {
        let parsed = ClapAttr::parse_all(attrs);

        for attr in &parsed {
            if let Some(AttrValue::Call(_)) = &attr.value {
                continue;
            }

            let kind = match &attr.magic {
                Some(MagicAttrName::FromGlobal) => {
                    if attr.value.is_some() {
                        let expr = attr.value_or_abort();
                        abort!(expr, "attribute `{}` does not accept a value", attr.name);
                    }
                    let ty = Sp::call_site(Ty::Other);
                    let kind = Sp::new(Kind::FromGlobal(ty), attr.name.clone().span());
                    Some(kind)
                }
                Some(MagicAttrName::Subcommand) if attr.value.is_none() => {
                    if attr.value.is_some() {
                        let expr = attr.value_or_abort();
                        abort!(expr, "attribute `{}` does not accept a value", attr.name);
                    }
                    let ty = Sp::call_site(Ty::Other);
                    let kind = Sp::new(Kind::Subcommand(ty), attr.name.clone().span());
                    Some(kind)
                }
                Some(MagicAttrName::ExternalSubcommand) if attr.value.is_none() => {
                    if attr.value.is_some() {
                        let expr = attr.value_or_abort();
                        abort!(expr, "attribute `{}` does not accept a value", attr.name);
                    }
                    let kind = Sp::new(Kind::ExternalSubcommand, attr.name.clone().span());
                    Some(kind)
                }
                Some(MagicAttrName::Flatten) if attr.value.is_none() => {
                    if attr.value.is_some() {
                        let expr = attr.value_or_abort();
                        abort!(expr, "attribute `{}` does not accept a value", attr.name);
                    }
                    let kind = Sp::new(Kind::Flatten, attr.name.clone().span());
                    Some(kind)
                }
                Some(MagicAttrName::Skip) => {
                    let expr = attr.value.clone();
                    let kind = Sp::new(
                        Kind::Skip(expr, self.kind.attr_kind()),
                        attr.name.clone().span(),
                    );
                    Some(kind)
                }
                _ => None,
            };

            if let Some(kind) = kind {
                self.set_kind(kind);
            }
        }

        for attr in &parsed {
            let actual_attr_kind = *attr.kind.get();
            let expected_attr_kind = self.kind.attr_kind();
            match (actual_attr_kind, expected_attr_kind) {
                (AttrKind::Clap, _) | (AttrKind::StructOpt, _) => {
                    self.deprecations.push(Deprecation::attribute(
                        "4.0.0",
                        actual_attr_kind,
                        expected_attr_kind,
                        attr.kind.span(),
                    ));
                }

                _ if attr.kind != expected_attr_kind => {
                    abort!(
                        attr.kind.span(),
                        "Expected `{}` attribute instead of `{}`",
                        expected_attr_kind.as_str(),
                        actual_attr_kind.as_str()
                    );
                }

                _ => {}
            }

            if let Some(AttrValue::Call(tokens)) = &attr.value {
                // Force raw mode with method call syntax
                self.push_method(attr.name.clone(), quote!(#(#tokens),*));
                continue;
            }

            match &attr.magic {
                Some(MagicAttrName::Short) if attr.value.is_none() => {
                    self.push_method(
                        attr.name.clone(),
                        self.name.clone().translate_char(*self.casing),
                    );
                }

                Some(MagicAttrName::Long) if attr.value.is_none() => {
                    self.push_method(attr.name.clone(), self.name.clone().translate(*self.casing));
                }

                #[cfg(not(feature = "unstable-v5"))]
                Some(MagicAttrName::ValueParser) if attr.value.is_none() => {
                    self.deprecations.push(Deprecation {
                        span: attr.name.span(),
                        id: "bare_value_parser",
                        version: "4.0.0",
                        description: "`#[arg(value_parser)]` is now the default and is no longer needed`".to_owned(),
                    });
                    self.value_parser = Some(ValueParser::Implicit(attr.name.clone()));
                }

                #[cfg(not(feature = "unstable-v5"))]
                Some(MagicAttrName::Action) if attr.value.is_none() => {
                    self.deprecations.push(Deprecation {
                        span: attr.name.span(),
                        id: "bare_action",
                        version: "4.0.0",
                        description: "`#[arg(action)]` is now the default and is no longer needed`".to_owned(),
                    });
                    self.action = Some(Action::Implicit(attr.name.clone()));
                }

                Some(MagicAttrName::Env) if attr.value.is_none() => {
                    self.push_method(
                        attr.name.clone(),
                        self.name.clone().translate(*self.env_casing),
                    );
                }

                Some(MagicAttrName::ValueEnum) if attr.value.is_none() => {
                    self.is_enum = true
                }

                Some(MagicAttrName::VerbatimDocComment) if attr.value.is_none() => {
                    self.verbatim_doc_comment = true
                }

                Some(MagicAttrName::About) if attr.value.is_none() => {
                    if let Some(method) =
                        Method::from_env(attr.name.clone(), "CARGO_PKG_DESCRIPTION")
                    {
                        self.methods.push(method);
                    }
                }

                Some(MagicAttrName::Author) if attr.value.is_none() => {
                    if let Some(method) = Method::from_env(attr.name.clone(), "CARGO_PKG_AUTHORS") {
                        self.methods.push(method);
                    }
                }

                Some(MagicAttrName::Version) if attr.value.is_none() => {
                    if let Some(method) = Method::from_env(attr.name.clone(), "CARGO_PKG_VERSION") {
                        self.methods.push(method);
                    }
                }

                Some(MagicAttrName::DefaultValueT) => {
                    let ty = if let Some(ty) = self.ty.as_ref() {
                        ty
                    } else {
                        abort!(
                            attr.name.clone(),
                            "#[arg(default_value_t)] (without an argument) can be used \
                            only on field level";

                            note = "see \
                                https://github.com/clap-rs/clap/blob/master/examples/derive_ref/README.md#magic-attributes")
                    };

                    let val = if let Some(expr) = &attr.value {
                        quote!(#expr)
                    } else {
                        quote!(<#ty as ::std::default::Default>::default())
                    };

                    let val = if parsed
                        .iter()
                        .any(|a| a.magic == Some(MagicAttrName::ValueEnum))
                    {
                        quote_spanned!(attr.name.clone().span()=> {
                            {
                                let val: #ty = #val;
                                clap::ValueEnum::to_possible_value(&val).unwrap().get_name().to_owned()
                            }
                        })
                    } else {
                        quote_spanned!(attr.name.clone().span()=> {
                            {
                                let val: #ty = #val;
                                ::std::string::ToString::to_string(&val)
                            }
                        })
                    };

                    let raw_ident = Ident::new("default_value", attr.name.clone().span());
                    self.methods.push(Method::new(raw_ident, val));
                }

                Some(MagicAttrName::DefaultValuesT) => {
                    let ty = if let Some(ty) = self.ty.as_ref() {
                        ty
                    } else {
                        abort!(
                            attr.name.clone(),
                            "#[arg(default_values_t)] (without an argument) can be used \
                            only on field level";

                            note = "see \
                                https://github.com/clap-rs/clap/blob/master/examples/derive_ref/README.md#magic-attributes")
                    };
                    let expr = attr.value_or_abort();

                    let container_type = Ty::from_syn_ty(ty);
                    if *container_type != Ty::Vec {
                        abort!(
                            attr.name.clone(),
                            "#[arg(default_values_t)] can be used only on Vec types";

                            note = "see \
                                https://github.com/clap-rs/clap/blob/master/examples/derive_ref/README.md#magic-attributes")
                    }
                    let inner_type = inner_type(ty);

                    // Use `Borrow<#inner_type>` so we accept `&Vec<#inner_type>` and
                    // `Vec<#inner_type>`.
                    let val = if parsed
                        .iter()
                        .any(|a| a.magic == Some(MagicAttrName::ValueEnum))
                    {
                        quote_spanned!(attr.name.clone().span()=> {
                            {
                                fn iter_to_vals<T>(iterable: impl IntoIterator<Item = T>) -> impl Iterator<Item=String>
                                where
                                    T: ::std::borrow::Borrow<#inner_type>
                                {
                                    iterable
                                        .into_iter()
                                        .map(|val| {
                                            clap::ValueEnum::to_possible_value(val.borrow()).unwrap().get_name().to_owned()
                                        })
                                }

                                iter_to_vals(#expr)
                            }
                        })
                    } else {
                        quote_spanned!(attr.name.clone().span()=> {
                            {
                                fn iter_to_vals<T>(iterable: impl IntoIterator<Item = T>) -> Vec<String>
                                where
                                    T: ::std::borrow::Borrow<#inner_type>
                                {
                                    iterable.into_iter().map(|val| val.borrow().to_string()).collect()

                                }

                                iter_to_vals(#expr)
                            }
                        })
                    };

                    self.methods.push(Method::new(
                        Ident::new("default_values", attr.name.clone().span()),
                        val,
                    ));
                }

                Some(MagicAttrName::DefaultValueOsT) => {
                    let ty = if let Some(ty) = self.ty.as_ref() {
                        ty
                    } else {
                        abort!(
                            attr.name.clone(),
                            "#[arg(default_value_os_t)] (without an argument) can be used \
                            only on field level";

                            note = "see \
                                https://github.com/clap-rs/clap/blob/master/examples/derive_ref/README.md#magic-attributes")
                    };

                    let val = if let Some(expr) = &attr.value {
                        quote!(#expr)
                    } else {
                        quote!(<#ty as ::std::default::Default>::default())
                    };

                    let val = if parsed
                        .iter()
                        .any(|a| a.magic == Some(MagicAttrName::ValueEnum))
                    {
                        quote_spanned!(attr.name.clone().span()=> {
                            {
                                let val: #ty = #val;
                                clap::ValueEnum::to_possible_value(&val).unwrap().get_name().to_owned()
                            }
                        })
                    } else {
                        quote_spanned!(attr.name.clone().span()=> {
                            {
                                let val: #ty = #val;
                                ::std::ffi::OsString::from(val)
                            }
                        })
                    };

                    let raw_ident = Ident::new("default_value", attr.name.clone().span());
                    self.methods.push(Method::new(raw_ident, val));
                }

                Some(MagicAttrName::DefaultValuesOsT) => {
                    let ty = if let Some(ty) = self.ty.as_ref() {
                        ty
                    } else {
                        abort!(
                            attr.name.clone(),
                            "#[arg(default_values_os_t)] (without an argument) can be used \
                            only on field level";

                            note = "see \
                                https://github.com/clap-rs/clap/blob/master/examples/derive_ref/README.md#magic-attributes")
                    };
                    let expr = attr.value_or_abort();

                    let container_type = Ty::from_syn_ty(ty);
                    if *container_type != Ty::Vec {
                        abort!(
                            attr.name.clone(),
                            "#[arg(default_values_os_t)] can be used only on Vec types";

                            note = "see \
                                https://github.com/clap-rs/clap/blob/master/examples/derive_ref/README.md#magic-attributes")
                    }
                    let inner_type = inner_type(ty);

                    // Use `Borrow<#inner_type>` so we accept `&Vec<#inner_type>` and
                    // `Vec<#inner_type>`.
                    let val = if parsed
                        .iter()
                        .any(|a| a.magic == Some(MagicAttrName::ValueEnum))
                    {
                        quote_spanned!(attr.name.clone().span()=> {
                            {
                                fn iter_to_vals<T>(iterable: impl IntoIterator<Item = T>) -> impl Iterator<Item=String>
                                where
                                    T: ::std::borrow::Borrow<#inner_type>
                                {
                                    iterable
                                        .into_iter()
                                        .map(|val| {
                                            clap::ValueEnum::to_possible_value(val.borrow()).unwrap().get_name().to_owned()
                                        })
                                }

                                iter_to_vals(#expr)
                            }
                        })
                    } else {
                        quote_spanned!(attr.name.clone().span()=> {
                            {
                                fn iter_to_vals<T>(iterable: impl IntoIterator<Item = T>) -> Vec<::std::ffi::OsString>
                                where
                                    T: ::std::borrow::Borrow<#inner_type>
                                {
                                    iterable.into_iter().map(|val| val.borrow().into()).collect()

                                }

                                iter_to_vals(#expr)
                            }
                        })
                    };

                    self.methods.push(Method::new(
                        Ident::new("default_values", attr.name.clone().span()),
                        val,
                    ));
                }

                Some(MagicAttrName::NextDisplayOrder) => {
                    let expr = attr.value_or_abort();
                    self.next_display_order = Some(Method::new(attr.name.clone(), quote!(#expr)));
                }

                Some(MagicAttrName::NextHelpHeading) => {
                    let expr = attr.value_or_abort();
                    self.next_help_heading = Some(Method::new(attr.name.clone(), quote!(#expr)));
                }

                Some(MagicAttrName::RenameAll) => {
                    let lit = attr.lit_str_or_abort();
                    self.casing = CasingStyle::from_lit(lit);
                }

                Some(MagicAttrName::RenameAllEnv) => {
                    let lit = attr.lit_str_or_abort();
                    self.env_casing = CasingStyle::from_lit(lit);
                }

                None
                // Magic only for the default, otherwise just forward to the builder
                | Some(MagicAttrName::Short)
                | Some(MagicAttrName::Long)
                | Some(MagicAttrName::Env)
                | Some(MagicAttrName::About)
                | Some(MagicAttrName::Author)
                | Some(MagicAttrName::Version)
                 => {
                    let expr = attr.value_or_abort();
                    self.push_method(attr.name.clone(), expr);
                }

                // Magic only for the default, otherwise just forward to the builder
                #[cfg(not(feature = "unstable-v5"))]
                Some(MagicAttrName::ValueParser) | Some(MagicAttrName::Action) => {
                    let expr = attr.value_or_abort();
                    self.push_method(attr.name.clone(), expr);
                }

                // Directives that never receive a value
                Some(MagicAttrName::ValueEnum)
                | Some(MagicAttrName::VerbatimDocComment) => {
                    let expr = attr.value_or_abort();
                    abort!(expr, "attribute `{}` does not accept a value", attr.name);
                }

                // Kinds
                Some(MagicAttrName::FromGlobal)
                | Some(MagicAttrName::Subcommand)
                | Some(MagicAttrName::ExternalSubcommand)
                | Some(MagicAttrName::Flatten)
                | Some(MagicAttrName::Skip) => {
                }
            }
        }
    }

    fn push_doc_comment(&mut self, attrs: &[Attribute], name: &str) {
        use syn::Lit::*;
        use syn::Meta::*;

        let comment_parts: Vec<_> = attrs
            .iter()
            .filter(|attr| attr.path.is_ident("doc"))
            .filter_map(|attr| {
                if let Ok(NameValue(MetaNameValue { lit: Str(s), .. })) = attr.parse_meta() {
                    Some(s.value())
                } else {
                    // non #[doc = "..."] attributes are not our concern
                    // we leave them for rustc to handle
                    None
                }
            })
            .collect();

        self.doc_comment = process_doc_comment(comment_parts, name, !self.verbatim_doc_comment);
    }

    fn set_kind(&mut self, kind: Sp<Kind>) {
        match (self.kind.get(), kind.get()) {
            (Kind::Arg(_), Kind::FromGlobal(_))
            | (Kind::Arg(_), Kind::Subcommand(_))
            | (Kind::Arg(_), Kind::Flatten)
            | (Kind::Arg(_), Kind::Skip(_, _))
            | (Kind::Command(_), Kind::Subcommand(_))
            | (Kind::Command(_), Kind::Flatten)
            | (Kind::Command(_), Kind::Skip(_, _))
            | (Kind::Command(_), Kind::ExternalSubcommand)
            | (Kind::Value(_), Kind::Skip(_, _)) => {
                self.kind = kind;
            }

            (_, _) => {
                let old = self.kind.name();
                let new = kind.name();
                abort!(kind.span(), "`{}` cannot be used with `{}`", new, old);
            }
        }
    }

    pub fn find_default_method(&self) -> Option<&Method> {
        self.methods
            .iter()
            .find(|m| m.name == "default_value" || m.name == "default_value_os")
    }

    /// generate methods from attributes on top of struct or enum
    pub fn initial_top_level_methods(&self) -> TokenStream {
        let next_display_order = self.next_display_order.as_ref().into_iter();
        let next_help_heading = self.next_help_heading.as_ref().into_iter();
        quote!(
            #(#next_display_order)*
            #(#next_help_heading)*
        )
    }

    pub fn final_top_level_methods(&self) -> TokenStream {
        let methods = &self.methods;
        let doc_comment = &self.doc_comment;

        quote!( #(#doc_comment)* #(#methods)*)
    }

    /// generate methods on top of a field
    pub fn field_methods(&self, supports_long_help: bool) -> proc_macro2::TokenStream {
        let methods = &self.methods;
        match supports_long_help {
            true => {
                let doc_comment = &self.doc_comment;
                quote!( #(#doc_comment)* #(#methods)* )
            }
            false => {
                let doc_comment = self
                    .doc_comment
                    .iter()
                    .filter(|mth| mth.name != "long_help");
                quote!( #(#doc_comment)* #(#methods)* )
            }
        }
    }

    pub fn deprecations(&self) -> proc_macro2::TokenStream {
        let deprecations = &self.deprecations;
        quote!( #(#deprecations)* )
    }

    pub fn next_display_order(&self) -> TokenStream {
        let next_display_order = self.next_display_order.as_ref().into_iter();
        quote!( #(#next_display_order)* )
    }

    pub fn next_help_heading(&self) -> TokenStream {
        let next_help_heading = self.next_help_heading.as_ref().into_iter();
        quote!( #(#next_help_heading)* )
    }

    pub fn id(&self) -> TokenStream {
        self.name.clone().raw()
    }

    pub fn cased_name(&self) -> TokenStream {
        self.name.clone().translate(*self.casing)
    }

    pub fn value_name(&self) -> TokenStream {
        self.name.clone().translate(CasingStyle::ScreamingSnake)
    }

    pub fn value_parser(&self, field_type: &Type) -> Method {
        self.value_parser
            .clone()
            .map(|p| {
                let inner_type = inner_type(field_type);
                p.resolve(inner_type)
            })
            .unwrap_or_else(|| {
                let inner_type = inner_type(field_type);
                if let Some(action) = self.action.as_ref() {
                    let span = action.span();
                    default_value_parser(inner_type, span)
                } else {
                    let span = self
                        .action
                        .as_ref()
                        .map(|a| a.span())
                        .unwrap_or_else(|| self.kind.span());
                    default_value_parser(inner_type, span)
                }
            })
    }

    pub fn action(&self, field_type: &Type) -> Method {
        self.action
            .clone()
            .map(|p| p.resolve(field_type))
            .unwrap_or_else(|| {
                if let Some(value_parser) = self.value_parser.as_ref() {
                    let span = value_parser.span();
                    default_action(field_type, span)
                } else {
                    let span = self
                        .value_parser
                        .as_ref()
                        .map(|a| a.span())
                        .unwrap_or_else(|| self.kind.span());
                    default_action(field_type, span)
                }
            })
    }

    pub fn kind(&self) -> Sp<Kind> {
        self.kind.clone()
    }

    pub fn is_positional(&self) -> bool {
        self.is_positional
    }

    pub fn casing(&self) -> Sp<CasingStyle> {
        self.casing
    }

    pub fn env_casing(&self) -> Sp<CasingStyle> {
        self.env_casing
    }

    pub fn has_explicit_methods(&self) -> bool {
        self.methods
            .iter()
            .any(|m| m.name != "help" && m.name != "long_help")
    }
}

#[derive(Clone)]
enum ValueParser {
    Explicit(Method),
    #[cfg(not(feature = "unstable-v5"))]
    Implicit(Ident),
}

impl ValueParser {
    fn resolve(self, _inner_type: &Type) -> Method {
        match self {
            Self::Explicit(method) => method,
            #[cfg(not(feature = "unstable-v5"))]
            Self::Implicit(ident) => default_value_parser(_inner_type, ident.span()),
        }
    }

    fn span(&self) -> Span {
        match self {
            Self::Explicit(method) => method.name.span(),
            #[cfg(not(feature = "unstable-v5"))]
            Self::Implicit(ident) => ident.span(),
        }
    }
}

fn default_value_parser(inner_type: &Type, span: Span) -> Method {
    let func = Ident::new("value_parser", span);
    Method::new(
        func,
        quote_spanned! { span=>
            clap::value_parser!(#inner_type)
        },
    )
}

#[derive(Clone)]
pub enum Action {
    Explicit(Method),
    #[cfg(not(feature = "unstable-v5"))]
    Implicit(Ident),
}

impl Action {
    pub fn resolve(self, _field_type: &Type) -> Method {
        match self {
            Self::Explicit(method) => method,
            #[cfg(not(feature = "unstable-v5"))]
            Self::Implicit(ident) => default_action(_field_type, ident.span()),
        }
    }

    pub fn span(&self) -> Span {
        match self {
            Self::Explicit(method) => method.name.span(),
            #[cfg(not(feature = "unstable-v5"))]
            Self::Implicit(ident) => ident.span(),
        }
    }
}

fn default_action(field_type: &Type, span: Span) -> Method {
    let ty = Ty::from_syn_ty(field_type);
    let args = match *ty {
        Ty::Vec | Ty::OptionVec => {
            quote_spanned! { span=>
                clap::ArgAction::Append
            }
        }
        Ty::Option | Ty::OptionOption => {
            quote_spanned! { span=>
                clap::ArgAction::Set
            }
        }
        _ => {
            if is_simple_ty(field_type, "bool") {
                quote_spanned! { span=>
                    clap::ArgAction::SetTrue
                }
            } else {
                quote_spanned! { span=>
                    clap::ArgAction::Set
                }
            }
        }
    };

    let func = Ident::new("action", span);
    Method::new(func, args)
}

#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum Kind {
    Arg(Sp<Ty>),
    Command(Sp<Ty>),
    Value(Sp<Ty>),
    FromGlobal(Sp<Ty>),
    Subcommand(Sp<Ty>),
    Flatten,
    Skip(Option<AttrValue>, AttrKind),
    ExternalSubcommand,
}

impl Kind {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Arg(_) => "arg",
            Self::Command(_) => "command",
            Self::Value(_) => "value",
            Self::FromGlobal(_) => "from_global",
            Self::Subcommand(_) => "subcommand",
            Self::Flatten => "flatten",
            Self::Skip(_, _) => "skip",
            Self::ExternalSubcommand => "external_subcommand",
        }
    }

    pub fn attr_kind(&self) -> AttrKind {
        match self {
            Self::Arg(_) => AttrKind::Arg,
            Self::Command(_) => AttrKind::Command,
            Self::Value(_) => AttrKind::Value,
            Self::FromGlobal(_) => AttrKind::Arg,
            Self::Subcommand(_) => AttrKind::Command,
            Self::Flatten => AttrKind::Command,
            Self::Skip(_, kind) => *kind,
            Self::ExternalSubcommand => AttrKind::Command,
        }
    }
}

#[derive(Clone)]
pub struct Method {
    name: Ident,
    args: TokenStream,
}

impl Method {
    pub fn new(name: Ident, args: TokenStream) -> Self {
        Method { name, args }
    }

    fn from_env(ident: Ident, env_var: &str) -> Option<Self> {
        let mut lit = match env::var(env_var) {
            Ok(val) => {
                if val.is_empty() {
                    return None;
                }
                LitStr::new(&val, ident.span())
            }
            Err(_) => {
                abort!(ident,
                    "cannot derive `{}` from Cargo.toml", ident;
                    note = "`{}` environment variable is not set", env_var;
                    help = "use `{} = \"...\"` to set {} manually", ident, ident;
                );
            }
        };

        if ident == "author" {
            let edited = process_author_str(&lit.value());
            lit = LitStr::new(&edited, lit.span());
        }

        Some(Method::new(ident, quote!(#lit)))
    }

    pub(crate) fn args(&self) -> &TokenStream {
        &self.args
    }
}

impl ToTokens for Method {
    fn to_tokens(&self, ts: &mut proc_macro2::TokenStream) {
        let Method { ref name, ref args } = self;

        let tokens = quote!( .#name(#args) );

        tokens.to_tokens(ts);
    }
}

#[derive(Clone)]
pub struct Deprecation {
    pub span: Span,
    pub id: &'static str,
    pub version: &'static str,
    pub description: String,
}

impl Deprecation {
    fn attribute(version: &'static str, old: AttrKind, new: AttrKind, span: Span) -> Self {
        Self {
            span,
            id: "old_attribute",
            version,
            description: format!(
                "Attribute `#[{}(...)]` has been deprecated in favor of `#[{}(...)]`",
                old.as_str(),
                new.as_str()
            ),
        }
    }
}

impl ToTokens for Deprecation {
    fn to_tokens(&self, ts: &mut proc_macro2::TokenStream) {
        let tokens = if cfg!(feature = "deprecated") {
            let Deprecation {
                span,
                id,
                version,
                description,
            } = self;
            let span = *span;
            let id = Ident::new(id, span);

            quote_spanned!(span=> {
                #[deprecated(since = #version, note = #description)]
                fn #id() {}
                #id();
            })
        } else {
            quote!()
        };

        tokens.to_tokens(ts);
    }
}

/// replace all `:` with `, ` when not inside the `<>`
///
/// `"author1:author2:author3" => "author1, author2, author3"`
/// `"author1 <http://website1.com>:author2" => "author1 <http://website1.com>, author2"
fn process_author_str(author: &str) -> String {
    let mut res = String::with_capacity(author.len());
    let mut inside_angle_braces = 0usize;

    for ch in author.chars() {
        if inside_angle_braces > 0 && ch == '>' {
            inside_angle_braces -= 1;
            res.push(ch);
        } else if ch == '<' {
            inside_angle_braces += 1;
            res.push(ch);
        } else if inside_angle_braces == 0 && ch == ':' {
            res.push_str(", ");
        } else {
            res.push(ch);
        }
    }

    res
}

/// Defines the casing for the attributes long representation.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CasingStyle {
    /// Indicate word boundaries with uppercase letter, excluding the first word.
    Camel,
    /// Keep all letters lowercase and indicate word boundaries with hyphens.
    Kebab,
    /// Indicate word boundaries with uppercase letter, including the first word.
    Pascal,
    /// Keep all letters uppercase and indicate word boundaries with underscores.
    ScreamingSnake,
    /// Keep all letters lowercase and indicate word boundaries with underscores.
    Snake,
    /// Keep all letters lowercase and remove word boundaries.
    Lower,
    /// Keep all letters uppercase and remove word boundaries.
    Upper,
    /// Use the original attribute name defined in the code.
    Verbatim,
}

impl CasingStyle {
    fn from_lit(name: &LitStr) -> Sp<Self> {
        use self::CasingStyle::*;

        let normalized = name.value().to_upper_camel_case().to_lowercase();
        let cs = |kind| Sp::new(kind, name.span());

        match normalized.as_ref() {
            "camel" | "camelcase" => cs(Camel),
            "kebab" | "kebabcase" => cs(Kebab),
            "pascal" | "pascalcase" => cs(Pascal),
            "screamingsnake" | "screamingsnakecase" => cs(ScreamingSnake),
            "snake" | "snakecase" => cs(Snake),
            "lower" | "lowercase" => cs(Lower),
            "upper" | "uppercase" => cs(Upper),
            "verbatim" | "verbatimcase" => cs(Verbatim),
            s => abort!(name, "unsupported casing: `{}`", s),
        }
    }
}

#[derive(Clone)]
pub enum Name {
    Derived(Ident),
    Assigned(TokenStream),
}

impl Name {
    pub fn raw(self) -> TokenStream {
        match self {
            Name::Assigned(tokens) => tokens,
            Name::Derived(ident) => {
                let s = ident.unraw().to_string();
                quote_spanned!(ident.span()=> #s)
            }
        }
    }

    pub fn translate(self, style: CasingStyle) -> TokenStream {
        use CasingStyle::*;

        match self {
            Name::Assigned(tokens) => tokens,
            Name::Derived(ident) => {
                let s = ident.unraw().to_string();
                let s = match style {
                    Pascal => s.to_upper_camel_case(),
                    Kebab => s.to_kebab_case(),
                    Camel => s.to_lower_camel_case(),
                    ScreamingSnake => s.to_shouty_snake_case(),
                    Snake => s.to_snake_case(),
                    Lower => s.to_snake_case().replace('_', ""),
                    Upper => s.to_shouty_snake_case().replace('_', ""),
                    Verbatim => s,
                };
                quote_spanned!(ident.span()=> #s)
            }
        }
    }

    pub fn translate_char(self, style: CasingStyle) -> TokenStream {
        use CasingStyle::*;

        match self {
            Name::Assigned(tokens) => quote!( (#tokens).chars().next().unwrap() ),
            Name::Derived(ident) => {
                let s = ident.unraw().to_string();
                let s = match style {
                    Pascal => s.to_upper_camel_case(),
                    Kebab => s.to_kebab_case(),
                    Camel => s.to_lower_camel_case(),
                    ScreamingSnake => s.to_shouty_snake_case(),
                    Snake => s.to_snake_case(),
                    Lower => s.to_snake_case(),
                    Upper => s.to_shouty_snake_case(),
                    Verbatim => s,
                };

                let s = s.chars().next().unwrap();
                quote_spanned!(ident.span()=> #s)
            }
        }
    }
}