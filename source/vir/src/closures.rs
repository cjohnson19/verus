use crate::ast::VirErr;
use crate::ast::{Expr, ExprX};
use crate::ast_visitor::expr_visitor_check;
use crate::messages::error;

/// Makes the following check:
///
///  1. The closure does not mutate any variable from outside the closure.
///     Such closures are currently unsupported.
///
/// TODO make this check as well:
///
///  2. If a variable is referenced from spec mode but not actually captured in
///     tracked/exec mode, then that variable cannot be mutable.
///     (This is actually easy to support, but we expect it might be confusing to the user.)

pub fn check_closure_well_formed(expr: &Expr, is_proof_fn: bool) -> Result<(), VirErr> {
    expr_visitor_check(expr, &mut |scope_map, expr| {
        match &expr.x {
            ExprX::VarLoc(ident) => {
                if !scope_map.contains_key(ident) {
                    // If this isn't in the scope_map, then the var must have been
                    // declared outside the closure.

                    Err(error(
                        &expr.span,
                        "Verus does not currently support closures capturing a mutable reference for variables of any mode",
                    ))
                } else {
                    Ok(())
                }
            }
            ExprX::Return(_) if is_proof_fn => {
                // TODO: supporting return inside proof_fn requires more support in lifetime.rs
                // because spec return values are currently erased to (),
                // causing a type mismatch with the closure's expected return type.
                // See test_ensures_fail_return_stmt in tests/proof_closures.rs.
                Err(error(&expr.span, "Verus does not yet support `return` inside `proof_fn`"))
            }
            _ => Ok(()),
        }
    })
}
