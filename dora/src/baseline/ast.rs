use std::collections::{HashMap, HashSet};

use crate::baseline::fct::{GcPoint, JitBaselineFct};
use crate::cpu::STACK_FRAME_ALIGNMENT;
use crate::mem;
use crate::ty::{BuiltinType, TypeList};
use crate::vm::{Fct, FctId, FctSrc, NodeMap, VM};
use dora_parser::ast;

mod codegen;
mod info;

pub fn compile<'a, 'ast: 'a>(
    vm: &'a VM<'ast>,
    fct: &Fct<'ast>,
    src: &'a mut FctSrc,
    cls_type_params: &TypeList,
    fct_type_params: &TypeList,
) -> JitBaselineFct {
    let mut jit_info = JitInfo::new();
    info::generate(
        vm,
        fct,
        src,
        &mut jit_info,
        cls_type_params,
        fct_type_params,
    );
    codegen::generate(
        vm,
        fct,
        src,
        &mut jit_info,
        cls_type_params,
        fct_type_params,
    )
}

#[derive(Copy, Clone, Debug)]
enum Arg<'ast> {
    Expr(&'ast ast::Expr, BuiltinType),
    Stack(i32, BuiltinType),
    SelfieNew(BuiltinType),
    Selfie(BuiltinType),
}

impl<'ast> Arg<'ast> {
    fn ty(&self) -> BuiltinType {
        match *self {
            Arg::Expr(_, ty) => ty,
            Arg::Stack(_, ty) => ty,
            Arg::Selfie(ty) => ty,
            Arg::SelfieNew(ty) => ty,
        }
    }

    fn is_selfie_new(&self) -> bool {
        match *self {
            Arg::SelfieNew(_) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Debug)]
struct CallSite<'ast> {
    callee: FctId,
    cls_type_params: TypeList,
    fct_type_params: TypeList,
    args: Vec<Arg<'ast>>,
    super_call: bool,
    return_type: BuiltinType,
}

struct JitInfo<'ast> {
    stacksize: i32, // size of local variables on stack

    map_csites: NodeMap<CallSite<'ast>>,
    map_templates: NodeMap<TemplateJitInfo<'ast>>,
}

impl<'ast> JitInfo<'ast> {
    fn stacksize(&self) -> i32 {
        self.stacksize
    }

    fn new() -> JitInfo<'ast> {
        JitInfo {
            stacksize: 0,

            map_csites: NodeMap::new(),
            map_templates: NodeMap::new(),
        }
    }
}

#[derive(Clone)]
struct TemplateJitInfo<'ast> {
    string_buffer_offset: i32,
    string_part_offset: i32,
    string_buffer_new: CallSite<'ast>,
    part_infos: Vec<TemplatePartJitInfo<'ast>>,
    string_buffer_to_string: CallSite<'ast>,
}

#[derive(Clone)]
struct TemplatePartJitInfo<'ast> {
    object_offset: Option<i32>,
    to_string: Option<CallSite<'ast>>,
    append: CallSite<'ast>,
}

struct StackFrame {
    all: HashSet<i32>,
    references: HashSet<i32>,
    scopes: Vec<StackScope>,
}

impl StackFrame {
    fn new() -> StackFrame {
        StackFrame {
            all: HashSet::new(),
            references: HashSet::new(),
            scopes: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.scopes.is_empty() && self.all.is_empty() && self.references.is_empty()
    }

    fn push_scope(&mut self) {
        self.scopes.push(StackScope::new());
    }

    fn add_var(&mut self, ty: BuiltinType, offset: i32) {
        if ty.reference_type() {
            assert!(self.references.insert(offset));
        }

        assert!(self.all.insert(offset));

        let scope = self.scopes.last_mut().expect("no active scope");
        scope.add_var(ty, offset);
    }

    fn pop_scope(&mut self) {
        let scope = self.scopes.pop().expect("no active scope");

        for (offset, ty) in scope.vars.into_iter() {
            if ty.reference_type() {
                assert!(self.references.remove(&offset));
            }

            assert!(self.all.remove(&offset));
        }
    }

    fn add_temp(&mut self, ty: BuiltinType, offset: i32) {
        if ty.reference_type() {
            assert!(self.references.insert(offset));
        }

        assert!(self.all.insert(offset));
    }

    fn free_temp(&mut self, ty: BuiltinType, offset: i32) {
        if ty.reference_type() {
            assert!(self.references.remove(&offset));
        }

        assert!(self.all.remove(&offset));
    }

    fn gcpoint(&self) -> GcPoint {
        let mut offsets = Vec::new();

        for &offset in &self.references {
            offsets.push(offset);
        }

        GcPoint::from_offsets(offsets)
    }
}

struct StackScope {
    vars: HashMap<i32, BuiltinType>,
}

impl StackScope {
    fn new() -> StackScope {
        StackScope {
            vars: HashMap::new(),
        }
    }

    fn add_var(&mut self, ty: BuiltinType, offset: i32) {
        assert!(self.vars.insert(offset, ty).is_none());
    }
}

#[derive(Copy, Clone, PartialEq, Eq, Hash)]
struct ManagedVar(usize);

#[derive(Copy, Clone)]
struct ManagedStackSlot {
    var: ManagedVar,
    offset: i32,
}

impl ManagedStackSlot {
    fn offset(&self) -> i32 {
        self.offset
    }
}

struct ManagedStackFrame {
    vars: HashMap<ManagedVar, (BuiltinType, i32)>,
    scopes: Vec<ManagedStackScope>,
    next_var: ManagedVar,

    free_slots: FreeSlots,
    stacksize: i32,
}

impl ManagedStackFrame {
    fn new() -> ManagedStackFrame {
        ManagedStackFrame {
            vars: HashMap::new(),
            scopes: Vec::new(),
            next_var: ManagedVar(0),

            free_slots: FreeSlots::new(),
            stacksize: 0,
        }
    }

    fn is_empty(&self) -> bool {
        self.scopes.is_empty() && self.vars.is_empty()
    }

    fn push_scope(&mut self) {
        self.scopes.push(ManagedStackScope::new());
    }

    fn pop_scope(&mut self, vm: &VM) {
        let scope = self.scopes.pop().expect("no active scope");

        for var in scope.vars.into_iter() {
            self.free(var, vm);
        }
    }

    fn add_scope(&mut self, ty: BuiltinType, vm: &VM) -> ManagedStackSlot {
        let var_and_offset = self.alloc(ty, vm);
        let scope = self.scopes.last_mut().expect("no active scope");
        scope.add_var(var_and_offset.var);

        var_and_offset
    }

    fn add_temp(&mut self, ty: BuiltinType, vm: &VM) -> ManagedStackSlot {
        self.alloc(ty, vm)
    }

    fn free_temp(&mut self, temp: ManagedStackSlot, vm: &VM) {
        self.free(temp.var, vm)
    }

    fn alloc(&mut self, ty: BuiltinType, vm: &VM) -> ManagedStackSlot {
        let var = self.next_var;
        self.next_var = ManagedVar(var.0 + 1);

        let (size, alignment) = if ty.is_nil() {
            (mem::ptr_width(), mem::ptr_width())
        } else {
            (ty.size(vm), ty.align(vm))
        };

        let alloc = self.free_slots.alloc(size as u32, alignment as u32);

        let offset = if let Some(free_start) = alloc {
            -(free_start as i32 + size)
        } else {
            self.extend_stack(size, alignment)
        };

        self.vars.insert(var, (ty, offset));
        ManagedStackSlot { var, offset }
    }

    fn extend_stack(&mut self, size: i32, alignment: i32) -> i32 {
        self.stacksize = mem::align_i32(self.stacksize as i32, alignment) + size;
        -self.stacksize
    }

    fn initial_stacksize(&mut self, size: i32) {
        assert!(self.stacksize == 0);
        self.stacksize = size;
    }

    fn free(&mut self, var: ManagedVar, vm: &VM) {
        if let Some((ty, offset)) = self.vars.remove(&var) {
            let size = if ty.is_nil() {
                mem::ptr_width()
            } else {
                ty.size(vm)
            };
            let start = -(offset + size);
            self.free_slots
                .free(FreeSlot::new(start as u32, size as u32));
        } else {
            panic!("var not found");
        }
    }

    fn gcpoint(&self) -> GcPoint {
        let mut offsets: Vec<i32> = Vec::new();

        for (_, (ty, offset)) in &self.vars {
            if ty.reference_type() {
                offsets.push(*offset);
            }
        }

        GcPoint::from_offsets(offsets)
    }

    fn stacksize(&self) -> i32 {
        mem::align_i32(self.stacksize, STACK_FRAME_ALIGNMENT as i32)
    }
}

struct ManagedStackScope {
    vars: Vec<ManagedVar>,
}

impl ManagedStackScope {
    fn new() -> ManagedStackScope {
        ManagedStackScope { vars: Vec::new() }
    }

    fn add_var(&mut self, var: ManagedVar) {
        self.vars.push(var);
    }
}

struct FreeSlots {
    slots: Vec<FreeSlot>,
}

impl FreeSlots {
    fn new() -> FreeSlots {
        FreeSlots { slots: Vec::new() }
    }

    fn free(&mut self, new: FreeSlot) {
        let slots = self.slots.len();

        for idx in 0..slots {
            let slot = self.slots[idx];

            if idx > 0 {
                debug_assert!(self.slots[idx - 1].end() < slot.start());
            }

            if new.end() < slot.start() {
                // insert before
                self.slots.insert(idx, new);
            } else if new.end() == slot.start() {
                // extend current slot from left
                self.slots[idx] = FreeSlot::new(new.start(), new.size() + slot.size());
            } else if slot.end() == new.start() {
                if idx + 1 < slots && self.slots[idx + 1].start() == new.end() {
                    // merge two slots
                    let left = slot;
                    let right = self.slots[idx + 1];

                    self.slots.remove(idx);

                    let size = right.end() - left.start();
                    self.slots[idx] = FreeSlot::new(left.start(), size);
                } else {
                    // extend current slot from right
                    self.slots[idx] = FreeSlot::new(slot.start(), slot.size() + new.size());

                    if idx + 1 < slots {
                        debug_assert!(self.slots[idx].end() < self.slots[idx + 1].start());
                    }
                }
            } else {
                // continue to next slot
                continue;
            }

            return;
        }

        self.slots.push(new);
    }

    fn alloc(&mut self, size: u32, alignment: u32) -> Option<u32> {
        let mut result = None;
        let mut best = u32::max_value();
        let slots = self.slots.len();

        for idx in 0..slots {
            let slot = self.slots[idx];

            if idx > 0 {
                debug_assert!(self.slots[idx - 1].end() < slot.start());
            }

            if slot.size() < size {
                continue;
            } else if slot.size() == size {
                if is_aligned(slot.start(), alignment) {
                    self.slots.remove(idx);
                    return Some(slot.start());
                }
            } else {
                let start = align(slot.start(), alignment);

                if start + size < slot.end() {
                    let gap_left = start - slot.start();
                    let gap_right = slot.end() - (start + size);
                    let gap = gap_left + gap_right;

                    if gap < best {
                        best = gap;
                        result = Some(idx);
                    }
                }
            }
        }

        if let Some(mut idx) = result {
            let slot = self.slots[idx];
            self.slots.remove(idx);
            let start = align(slot.start(), alignment);
            let gap_left = start - slot.start();
            let gap_right = slot.end() - (start + size);

            if gap_left > 0 {
                self.slots
                    .insert(idx, FreeSlot::new(slot.start(), gap_left));
                idx += 1;
            }

            if gap_right > 0 {
                self.slots
                    .insert(idx, FreeSlot::new(slot.end() - gap_right, gap_right));
            }

            Some(start)
        } else {
            None
        }
    }
}

fn is_aligned(value: u32, size: u32) -> bool {
    value % size == 0
}

fn align(value: u32, alignment: u32) -> u32 {
    (value * alignment + alignment - 1) / alignment
}

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
struct FreeSlot {
    start: u32,
    size: u32,
}

impl FreeSlot {
    fn new(start: u32, size: u32) -> FreeSlot {
        FreeSlot { start, size }
    }

    fn start(self) -> u32 {
        self.start
    }

    fn end(self) -> u32 {
        self.start + self.size
    }

    fn size(self) -> u32 {
        self.size
    }
}

#[cfg(test)]
mod tests {
    use super::{FreeSlot, FreeSlots};

    #[test]
    fn merge_free_slots() {
        let mut free_slots = FreeSlots::new();
        free_slots.free(FreeSlot::new(0, 2));
        free_slots.free(FreeSlot::new(8, 8));
        free_slots.free(FreeSlot::new(2, 2));
        free_slots.free(FreeSlot::new(4, 4));

        assert_eq!(free_slots.slots, vec![FreeSlot::new(0, 16)]);

        let mut free_slots = FreeSlots::new();
        free_slots.free(FreeSlot::new(4, 8));
        free_slots.free(FreeSlot::new(0, 2));
        free_slots.free(FreeSlot::new(2, 2));

        assert_eq!(free_slots.slots, vec![FreeSlot::new(0, 12)]);
    }

    #[test]
    fn alloc_free_slot() {
        let mut free_slots = FreeSlots::new();

        assert_eq!(free_slots.alloc(2, 2), None);
        free_slots.free(FreeSlot::new(0, 2));

        assert_eq!(free_slots.alloc(2, 2), Some(0));
        assert_eq!(free_slots.slots, Vec::new());

        free_slots.free(FreeSlot::new(0, 8));
        free_slots.free(FreeSlot::new(12, 4));
        assert_eq!(free_slots.alloc(4, 4), Some(12));
        assert_eq!(free_slots.slots, vec![FreeSlot::new(0, 8)]);
    }
}
