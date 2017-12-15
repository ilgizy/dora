use std::rc::Rc;

use ctxt::SemContext;
use driver::cmd::Args;
use gc::arena;
use gc::Address;
use gc::Collector;
use gc::root::get_rootset;
use gc::swiper::card::CardTable;
use gc::swiper::crossing::CrossingMap;
use gc::swiper::young::YoungGen;
use gc::swiper::old::OldGen;
use mem;

mod crossing;
pub mod young;
pub mod old;
pub mod card;

// determines size of young generation in heap
// young generation size = heap size / YOUNG_RATIO
const YOUNG_RATIO: u32 = 5;

// size of card is 128 bytes
// although stored in constant
// it is not really expected to change
pub const CARD_SIZE: usize = 512;
pub const CARD_SIZE_BITS: usize = 9;

// the number of collections an object remains in
// young generation
pub const PROMOTION_AGE: u32 = 4;

pub struct Swiper {
    heap: Region,

    young: YoungGen,
    old: OldGen,
    card_table: CardTable,
    crossing_map: Rc<CrossingMap>,

    card_table_offset: usize,
}

impl Swiper {
    pub fn new(args: &Args) -> Swiper {
        let heap_size = args.flag_heap_size.map(|s| *s).unwrap_or(32 * 1024 * 1024);

        // set heap size to multiple of page
        let heap_size = mem::page_align(heap_size);

        // determine sizes of young/old-gen
        let young_size = mem::page_align(heap_size / (YOUNG_RATIO as usize));
        let old_size = heap_size - young_size;

        // determine size for card table
        let card_size = mem::page_align(heap_size >> CARD_SIZE_BITS);

        // determine size for crossing map
        let crossing_size = mem::page_align(old_size >> CARD_SIZE_BITS);

        let alloc_size = heap_size + card_size + crossing_size;

        let ptr = arena::reserve(alloc_size).expect("could not reserve memory");
        let ptr = Address::from_ptr(ptr);

        let heap_start = ptr;
        let heap_end = ptr.offset(heap_size);

        // determine offset to card table (card table starts right after heap)
        // offset = card_table_start - (heap_start >> CARD_SIZE_BITS)
        let card_table_offset = heap_end.to_usize() - (heap_start.to_usize() >> CARD_SIZE_BITS);

        // determine boundaries for card table
        let card_start = heap_end;
        let card_end = card_start.offset(card_size);
        let card_table = CardTable::new(card_start, card_end, young_size);

        // determine boundaries for crossing map
        let crossing_start = card_end;
        let crossing_end = crossing_start.offset(crossing_size);
        let crossing_map = Rc::new(CrossingMap::new(crossing_start, crossing_end));

        // determine boundaries of young generation
        let young_start = heap_start;
        let young_end = young_start.offset(young_size);
        let young = YoungGen::new(young_start, young_end);

        // determine boundaries of old generation
        let old_start = heap_start.offset(young_size);
        let old_end = heap_end;
        let old = OldGen::new(old_start, old_end);

        Swiper {
            heap: Region::new(heap_start, heap_end),

            young: young,
            old: old,
            card_table: card_table,
            crossing_map: crossing_map,

            card_table_offset: card_table_offset,
        }
    }
}

impl Collector for Swiper {
    fn alloc_obj(&self, ctxt: &SemContext, size: usize) -> *const u8 {
        let ptr = self.young.alloc(size);

        if !ptr.is_null() {
            return ptr;
        }

        let rootset = get_rootset(ctxt);
        self.young.collect(
            ctxt,
            rootset,
            &self.card_table,
            &*self.crossing_map,
            &self.old,
        );

        self.young.alloc(size)
    }

    fn collect(&self, _: &SemContext) {
        unimplemented!();
    }

    fn needs_write_barrier(&self) -> bool {
        return true;
    }

    fn card_table_offset(&self) -> usize {
        self.card_table_offset
    }
}

pub struct Region {
    pub start: Address,
    pub end: Address,
}

impl Region {
    fn new(start: Address, end: Address) -> Region {
        Region {
            start: start,
            end: end,
        }
    }

    #[inline(always)]
    fn includes(&self, addr: Address) -> bool {
        self.start <= addr && addr < self.end
    }

    #[inline(always)]
    fn size(&self) -> usize {
        self.end.to_usize() - self.start.to_usize()
    }
}
