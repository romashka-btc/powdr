use std::{collections::HashMap, hash::Hash};

use bit_vec::BitVec;
use powdr_number::{FieldElement, KnownField};

use crate::witgen::{
    data_structures::finalizable_data::{ColumnLayout, CompactDataRef},
    machines::{LookupCell, MachineParts},
    EvalError, FixedData, MutableState, QueryCallback,
};

use super::{
    block_machine_processor::BlockMachineProcessor,
    compiler::{compile_effects, WitgenFunction},
    variable::Variable,
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
struct CacheKey {
    identity_id: u64,
    known_args: BitVec,
}

pub struct FunctionCache<'a, T: FieldElement> {
    /// The processor that generates the JIT code
    processor: BlockMachineProcessor<'a, T>,
    /// The cache of JIT functions. If the entry is None, we attempted to generate the function
    /// but failed.
    witgen_functions: HashMap<CacheKey, Option<WitgenFunction<T>>>,
    column_layout: ColumnLayout,
}

impl<'a, T: FieldElement> FunctionCache<'a, T> {
    pub fn new(
        fixed_data: &'a FixedData<'a, T>,
        parts: MachineParts<'a, T>,
        block_size: usize,
        latch_row: usize,
        metadata: ColumnLayout,
    ) -> Self {
        let processor =
            BlockMachineProcessor::new(fixed_data, parts.clone(), block_size, latch_row);

        FunctionCache {
            processor,
            column_layout: metadata,
            witgen_functions: HashMap::new(),
        }
    }

    /// Compiles the JIT function for the given identity and known arguments.
    /// Returns true if the function was successfully compiled.
    pub fn compile_cached<Q: QueryCallback<T>>(
        &mut self,
        mutable_state: &MutableState<'a, T, Q>,
        identity_id: u64,
        known_args: &BitVec,
    ) -> &Option<WitgenFunction<T>> {
        let cache_key = CacheKey {
            identity_id,
            known_args: known_args.clone(),
        };
        self.ensure_cache(mutable_state, &cache_key);
        self.witgen_functions.get(&cache_key).unwrap()
    }

    fn ensure_cache<Q: QueryCallback<T>>(
        &mut self,
        mutable_state: &MutableState<'a, T, Q>,
        cache_key: &CacheKey,
    ) {
        if self.witgen_functions.contains_key(cache_key) {
            return;
        }

        let f = match T::known_field() {
            // Currently, we only support the Goldilocks fields
            Some(KnownField::GoldilocksField) => {
                self.compile_witgen_function(mutable_state, cache_key)
            }
            _ => None,
        };
        assert!(self.witgen_functions.insert(cache_key.clone(), f).is_none())
    }

    fn compile_witgen_function<Q: QueryCallback<T>>(
        &self,
        mutable_state: &MutableState<'a, T, Q>,
        cache_key: &CacheKey,
    ) -> Option<WitgenFunction<T>> {
        log::trace!("Compiling JIT function for {:?}", cache_key);
        self.processor
            .generate_code(mutable_state, cache_key.identity_id, &cache_key.known_args)
            .ok()
            .map(|code| {
                log::trace!("Generated code ({} steps)", code.len());
                let known_inputs = cache_key
                    .known_args
                    .iter()
                    .enumerate()
                    .filter_map(|(i, b)| if b { Some(Variable::Param(i)) } else { None })
                    .collect::<Vec<_>>();

                log::trace!("Compiling effects...");

                compile_effects(
                    self.column_layout.first_column_id,
                    self.column_layout.column_count,
                    &known_inputs,
                    &code,
                )
                .unwrap()
            })
    }

    pub fn process_lookup_direct<'c, 'd, Q: QueryCallback<T>>(
        &self,
        mutable_state: &MutableState<'a, T, Q>,
        connection_id: u64,
        mut values: Vec<LookupCell<'c, T>>,
        data: CompactDataRef<'d, T>,
    ) -> Result<bool, EvalError<T>> {
        let known_args = values
            .iter()
            .map(|cell| cell.is_input())
            .collect::<BitVec>();

        let cache_key = CacheKey {
            identity_id: connection_id,
            known_args,
        };

        let f = self
            .witgen_functions
            .get(&cache_key)
            .expect("Need to call compile_cached() first!")
            .as_ref()
            .expect("compile_cached() returned false!");
        f.call(mutable_state, &mut values, data);

        Ok(true)
    }
}
