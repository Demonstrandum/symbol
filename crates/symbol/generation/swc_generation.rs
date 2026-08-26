use swc_core::common::{FileName, GLOBALS, Globals, Mark, SourceMap, sync::Lrc};
use swc_core::ecma::ast::{
    Accessibility, Class, ClassMember, Decl, DefaultDecl, EsVersion, Key, MemberExpr, MemberProp,
    Module, ModuleDecl, ModuleItem, Program, Stmt, TsModuleDecl, TsNamespaceBody,
};
use swc_core::ecma::codegen::{Config, Emitter, text_writer::JsWriter};
#[cfg(test)]
use swc_core::ecma::parser::EsSyntax;
use swc_core::ecma::parser::{Syntax, TsSyntax, parse_file_as_module};
use swc_core::ecma::transforms::base::{fixer::fixer, resolver};
use swc_core::ecma::transforms::module::{
    path::Resolver as ModuleResolver,
    umd::{Config as UmdConfig, FeatureFlag, umd},
};
use swc_core::ecma::transforms::typescript::strip;
use swc_core::ecma::visit::{VisitMut, VisitMutWith as _};

use super::GenerationError;

pub fn emit_typescript(source: &str, file_name: &str) -> Result<String, GenerationError> {
    let (source_map, module) = parse_typescript(source, file_name)?;
    emit_module(source_map, &module)
}

pub fn emit_javascript_module(source: &str, file_name: &str) -> Result<String, GenerationError> {
    let (source_map, module) = parse_typescript(source, file_name)?;
    let module = strip_typescript(module);
    emit_module(source_map, &module)
}

pub fn emit_umd(source: &str) -> Result<String, GenerationError> {
    let (source_map, module) = parse_typescript(source, "symbol_api")?;
    let mut program = GLOBALS.set(&Globals::default(), || {
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        Program::Module(module)
            .apply(resolver(unresolved_mark, top_level_mark, true))
            .apply(strip(unresolved_mark, top_level_mark))
            .apply(umd(
                Lrc::clone(&source_map),
                ModuleResolver::default(),
                unresolved_mark,
                UmdConfig::default(),
                FeatureFlag {
                    support_block_scoping: true,
                },
            ))
    });
    let mut renamer = UmdGlobalRenamer::default();
    program.visit_mut_with(&mut renamer);
    if renamer.renamed != 1 {
        return Err(GenerationError::Swc(format!(
            "expected one inferred UMD global, renamed {}",
            renamer.renamed
        )));
    }
    program = GLOBALS.set(&Globals::default(), || program.apply(fixer(None)));
    let Program::Module(module) = program else {
        return Err(GenerationError::Swc(
            "UMD transform returned a script".to_string(),
        ));
    };
    emit_module(source_map, &module)
}

pub fn emit_declarations(source: &str, file_name: &str) -> Result<String, GenerationError> {
    let (source_map, mut module) = parse_typescript(source, file_name)?;
    let mut declarations = 0_usize;
    retain_declaration_items(&mut module.body, false, &mut declarations);
    if declarations == 0 {
        return Err(GenerationError::Swc(
            "TypeScript template has no exported declarations".to_string(),
        ));
    }
    emit_module(source_map, &module)
}

fn retain_declaration_items(items: &mut Vec<ModuleItem>, ambient: bool, declarations: &mut usize) {
    items.retain_mut(|item| match item {
        ModuleItem::ModuleDecl(ModuleDecl::ExportDecl(export)) => {
            let retained = declaration_only(&mut export.decl, ambient, declarations);
            *declarations += usize::from(retained);
            retained
        }
        ModuleItem::ModuleDecl(ModuleDecl::ExportDefaultDecl(export)) => {
            match &mut export.decl {
                DefaultDecl::Class(class) => strip_class(&mut class.class),
                DefaultDecl::Fn(function) => function.function.body = None,
                DefaultDecl::TsInterfaceDecl(_) => {}
            }
            *declarations += 1;
            true
        }
        ModuleItem::ModuleDecl(
            ModuleDecl::Import(_)
            | ModuleDecl::ExportNamed(_)
            | ModuleDecl::ExportAll(_)
            | ModuleDecl::TsImportEquals(_)
            | ModuleDecl::TsNamespaceExport(_),
        ) => true,
        ModuleItem::Stmt(Stmt::Decl(declaration)) => {
            retain_private_type_dependency(declaration, ambient, declarations)
        }
        ModuleItem::Stmt(_)
        | ModuleItem::ModuleDecl(
            ModuleDecl::ExportDefaultExpr(_) | ModuleDecl::TsExportAssignment(_),
        ) => false,
    });
}

fn declaration_only(declaration: &mut Decl, ambient: bool, declarations: &mut usize) -> bool {
    match declaration {
        Decl::Var(variable) => {
            variable.declare = !ambient;
            for declarator in &mut variable.decls {
                declarator.init = None;
            }
            true
        }
        Decl::Fn(function) => {
            function.declare = !ambient;
            function.function.body = None;
            true
        }
        Decl::Class(class) => {
            class.declare = !ambient;
            strip_class(&mut class.class);
            true
        }
        Decl::TsEnum(enumeration) => {
            enumeration.declare = !ambient;
            true
        }
        Decl::TsModule(module) => {
            strip_namespace(module, ambient, declarations);
            true
        }
        Decl::TsInterface(_) | Decl::TsTypeAlias(_) => true,
        Decl::Using(_) => false,
    }
}

fn retain_private_type_dependency(
    declaration: &mut Decl,
    ambient: bool,
    declarations: &mut usize,
) -> bool {
    match declaration {
        Decl::TsInterface(_) | Decl::TsTypeAlias(_) => true,
        Decl::TsModule(module) => {
            strip_namespace(module, ambient, declarations);
            true
        }
        Decl::Var(_) | Decl::Fn(_) | Decl::Class(_) | Decl::TsEnum(_) | Decl::Using(_) => false,
    }
}

fn strip_namespace(namespace: &mut TsModuleDecl, ambient: bool, declarations: &mut usize) {
    namespace.declare = !ambient;
    if let Some(body) = &mut namespace.body {
        strip_namespace_body(body, declarations);
    }
}

fn strip_namespace_body(body: &mut TsNamespaceBody, declarations: &mut usize) {
    match body {
        TsNamespaceBody::TsModuleBlock(block) => {
            retain_declaration_items(&mut block.body, true, declarations);
        }
        TsNamespaceBody::TsNamespaceDecl(namespace) => {
            namespace.declare = false;
            strip_namespace_body(&mut namespace.body, declarations);
        }
    }
}

fn strip_class(class: &mut Class) {
    class.body.retain_mut(|member| match member {
        ClassMember::Constructor(constructor) => {
            if is_private(constructor.accessibility) {
                false
            } else {
                constructor.body = None;
                true
            }
        }
        ClassMember::Method(method) => {
            if is_private(method.accessibility) {
                false
            } else {
                method.function.body = None;
                true
            }
        }
        ClassMember::ClassProp(property) => {
            if is_private(property.accessibility) {
                false
            } else {
                property.value = None;
                true
            }
        }
        ClassMember::AutoAccessor(accessor) => {
            if is_private(accessor.accessibility) || matches!(accessor.key, Key::Private(_)) {
                false
            } else {
                accessor.value = None;
                true
            }
        }
        ClassMember::TsIndexSignature(_) => true,
        ClassMember::PrivateMethod(_)
        | ClassMember::PrivateProp(_)
        | ClassMember::Empty(_)
        | ClassMember::StaticBlock(_) => false,
    });
}

const fn is_private(accessibility: Option<Accessibility>) -> bool {
    matches!(accessibility, Some(Accessibility::Private))
}

fn strip_typescript(module: Module) -> Module {
    GLOBALS.set(&Globals::default(), || {
        let unresolved_mark = Mark::new();
        let top_level_mark = Mark::new();
        let program = Program::Module(module)
            .apply(resolver(unresolved_mark, top_level_mark, true))
            .apply(strip(unresolved_mark, top_level_mark))
            .apply(fixer(None));
        let Program::Module(module) = program else {
            unreachable!("TypeScript module transforms preserve module shape");
        };
        module
    })
}

fn parse_typescript(
    source: &str,
    file_name: &str,
) -> Result<(Lrc<SourceMap>, Module), GenerationError> {
    parse_module(source, file_name, Syntax::Typescript(TsSyntax::default()))
}

fn parse_module(
    source: &str,
    file_name: &str,
    syntax: Syntax,
) -> Result<(Lrc<SourceMap>, Module), GenerationError> {
    let source_map = Lrc::<SourceMap>::default();
    let source_file = source_map.new_source_file(
        FileName::Custom(file_name.to_string()).into(),
        source.to_string(),
    );
    let mut recovered = Vec::new();
    let module = parse_file_as_module(
        &source_file,
        syntax,
        EsVersion::Es2020,
        None,
        &mut recovered,
    )
    .map_err(|error| GenerationError::Swc(format!("{file_name}: {error:?}")))?;
    if !recovered.is_empty() {
        return Err(GenerationError::Swc(format!(
            "{file_name}: recovered parser errors: {recovered:?}"
        )));
    }
    Ok((source_map, module))
}

fn emit_module(source_map: Lrc<SourceMap>, module: &Module) -> Result<String, GenerationError> {
    let mut bytes = Vec::new();
    {
        let writer = JsWriter::new(Lrc::clone(&source_map), "\n", &mut bytes, None);
        let mut emitter = Emitter {
            cfg: Config::default().with_target(EsVersion::Es2020),
            comments: None,
            cm: source_map,
            wr: writer,
        };
        emitter
            .emit_module(module)
            .map_err(|error| GenerationError::Swc(error.to_string()))?;
    }
    String::from_utf8(bytes)
        .map_err(|error| GenerationError::Swc(format!("SWC emitted invalid UTF-8: {error}")))
}

#[derive(Default)]
struct UmdGlobalRenamer {
    renamed: u32,
}

impl VisitMut for UmdGlobalRenamer {
    fn visit_mut_member_expr(&mut self, member: &mut MemberExpr) {
        member.visit_mut_children_with(self);
        let MemberProp::Ident(property) = &mut member.prop else {
            return;
        };
        if property.sym == *"symbolApi" || property.sym == *"symbolAPI" {
            property.sym = "SymbolAPI".into();
            self.renamed += 1;
        }
    }
}

#[cfg(test)]
pub fn parse_es_module_for_test(source: &str) -> Result<Module, GenerationError> {
    parse_module(source, "test.js", Syntax::Es(EsSyntax::default())).map(|(_, module)| module)
}

#[cfg(test)]
pub fn parse_ts_module_for_test(source: &str) -> Result<Module, GenerationError> {
    parse_typescript(source, "test.ts").map(|(_, module)| module)
}
