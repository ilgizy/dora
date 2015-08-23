use parser::ast::*;
use parser::ast::Elem::*;
use parser::ast::Expr::*;
use parser::ast::Stmt::*;

use error::ParseError;

pub trait Visitor<'v> : Sized {
    fn visit_ast(&mut self, a: &'v Ast) {
        walk_ast(self, a)
    }

    fn visit_fct(&mut self, a: &'v Function) {
        walk_stmt(self, &a.block)
    }

    fn visit_param(&mut self, p: &'v Param) {
        walk_param(self, p);
    }

    fn visit_stmt(&mut self, s: &'v Stmt) {
        walk_stmt(self, s)
    }

    fn visit_expr(&mut self, e: &'v Expr) {
        walk_expr(self, e)
    }
}

pub fn walk_ast<'v, V: Visitor<'v>>(v: &mut V, a: &'v Ast) {
    for e in &a.elements {
        match *e {
            ElemFunction(ref f) => v.visit_fct(f),
            ElemUnknown => unreachable!()
        }
    }
}

pub fn walk_fct<'v, V: Visitor<'v>>(v: &mut V, f: &'v Function) {
    v.visit_stmt(&f.block)
}

pub fn walk_param<'v, V: Visitor<'v>>(v: &mut V, f: &'v Param) {

}

pub fn walk_stmt<'v, V: Visitor<'v>>(v: &mut V, s: &'v Stmt) {
    match *s {
        StmtVar(ref value) => {
            if let Some(ref e) = value.expr {
                v.visit_expr(e);
            }
        }

        StmtWhile(ref value) => {
            v.visit_expr(&value.cond);
            v.visit_stmt(&value.block);
        }

        StmtLoop(ref value) => {
            v.visit_stmt(&value.block);
        }

        StmtIf(ref value) => {
            v.visit_expr(&value.cond);
            v.visit_stmt(&value.then_block);

            if let Some(ref b) = value.else_block {
                v.visit_stmt(b);
            }
        }

        StmtExpr(ref value) => {
            v.visit_expr(&value.expr);
        }

        StmtBlock(ref value) => {
            for stmt in &value.stmts {
                v.visit_stmt(stmt);
            }
        }

        StmtReturn(ref value) => {
            if let Some(ref e) = value.expr {
                v.visit_expr(e);
            }
        }

        StmtBreak(_) => { }
        StmtContinue(_) => { }
    }
}

pub fn walk_expr<'v, V: Visitor<'v>>(v: &mut V, e: &'v Expr) {
    match *e {
        ExprUn(ref value) => {
            v.visit_expr(&value.opnd);
        }

        ExprBin(ref value) => {
            v.visit_expr(&value.lhs);
            v.visit_expr(&value.rhs);
        }

        ExprAssign(ref value) => {
            v.visit_expr(&value.lhs);
            v.visit_expr(&value.rhs);
        }

        ExprLitInt(_) => {}
        ExprLitStr(_) => {}
        ExprLitBool(_) => {}
        ExprIdent(_) => {}
    }
}
