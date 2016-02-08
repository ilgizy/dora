use std::collections::HashMap;

use ast::*;
use ast::visit::*;
use class::{self, ClassId};
use ctxt::*;
use error::msg::Msg;
use interner::Name;
use lexer::position::Position;
use sym::Sym::{self, SymClass};
use ty::BuiltinType;

pub fn check<'a, 'ast: 'a>(ctxt: &mut Context<'a, 'ast>) {
    let mut gdef = GlobalDef {
        ctxt: ctxt
    };

    gdef.visit_ast(ctxt.ast);
}

struct GlobalDef<'x, 'a: 'x, 'ast: 'a> {
    ctxt: &'x mut Context<'a, 'ast>
}

impl<'x, 'a, 'ast> Visitor<'ast> for GlobalDef<'x, 'a, 'ast> {
    fn visit_class(&mut self, c: &'ast Class) {
        let id = ClassId(self.ctxt.classes.len());
        let cls = class::Class {
            id: id,
            name: c.name,
            props: Vec::new()
        };

        self.ctxt.classes.push(Box::new(cls));

        if let Some(sym) = self.ctxt.sym.borrow_mut().insert(c.name, SymClass(id)) {
            report(self.ctxt, c.name, c.pos, sym);
        }
    }

    fn visit_fct(&mut self, f: &'ast Function) {
        let fct = FctContext {
            id: FctContextId(0),
            name: f.name,
            params_types: Vec::new(),
            return_type: BuiltinType::Unit,
            ast: Some(f),
            types: HashMap::new(),
            calls: HashMap::new(),
            defs: HashMap::new(),
            ir: None,
            tempsize: 0,
            localsize: 0,
            leaf: false,
            vars: Vec::new(),
            always_returns: false,
            code: FctCode::Uncompiled,
            stub: None,
        };

        if let Err(sym) = self.ctxt.add_function(fct) {
            report(self.ctxt, f.name, f.pos, sym);
        }
    }
}

fn report(ctxt: &Context, name: Name, pos: Position, sym: Sym) {
    let name = ctxt.interner.str(name).to_string();

    let msg = if sym.is_type() {
        Msg::ShadowType(name)
    } else {
        Msg::ShadowFunction(name)
    };

    ctxt.diag.borrow_mut().report(pos, msg);
}
