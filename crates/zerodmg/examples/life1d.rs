//! 1D Conway's Life — spacetime diagram on Game Boy
//!
//! An 8×1 wrapping grid evolves over time.
//! The screen shows an 8×8 block of tiles centered on the 160×144 display:
//!   each row = one generation, oldest at top, newest at bottom (rotating).
//!
//! Rules (Conway in 1D, 2 neighbors only, max count=2):
//!   Born:    count == 2 (both neighbors alive)
//!   Survive: count == 1 or count == 2
//!   Die:     count == 0
//!
//! Initial state: 0b00011000 = cells 3,4 alive (two in the middle of 8)
//!
//! Memory layout:
//!   0xC000 = current row (8 bytes: cells 0-7)
//!   0xC010 = write_row index (0-7): which BG row to write next generation to
//!
//! Rendering:
//!   8×8 BG tiles centered at tile offset (6,4) in the 20×18 BG map.
//!   (screen center = tile 10,9 → 8-wide block starts at tile 6, 8-tall at tile 1)
//!   Tile 0 = white (dead), Tile 1 = black (alive)

mod gb_asm;
use gb_asm::*;
use zerodmg_codes::instruction::Instruction;
use zerodmg_codes::instruction::prelude::*;

// 8-cell 1D grid
const CELLS: usize = 8;
const ROW_BASE: u16 = 0xC000; // current generation (8 bytes)
const WRITE_ROW: u16 = 0xC010; // which of the 8 display rows to write next (0-7)

// BG tilemap at 0x9800, 32 tiles wide
// We want an 8×8 block centered on 20×18:
//   horizontal: (20 - 8) / 2 = 6 → start at tile column 6
//   vertical:   (18 - 8) / 2 = 5 → start at tile row 5
const BG_COLS: u16 = 32; // BG map width
const TILE_COL: u16 = 6; // leftmost tile column of our 8-wide block
const TILE_ROW: u16 = 5; // topmost tile row of our 8-tall block (rows 5-12)

// Address in BG tilemap of cell (row, col) in our 8×8 block
fn bg_addr(row: u16, col: u16) -> u16 {
    0x9800 + (TILE_ROW + row) * BG_COLS + TILE_COL + col
}

fn main() {
    let rom = build_rom();
    std::fs::write("life1d.gb", &rom).expect("Failed to write life1d.gb");
    println!("Generated life1d.gb");
    println!("1D Conway's Life — 8×8 spacetime diagram");
    println!("Initial state: cells 3,4 alive (0b00011000)");
}

fn build_rom() -> Vec<u8> {
    let mut asm = Assembler::new();

    // ===== SETUP =====

    // Stack
    asm.inst(LD_16_IMMEDIATE(SP, 0xFFFE));

    // LCD on, BG on (LCDC = 0x91)
    asm.inst(LD_8_IMMEDIATE(A, 0x91))
        .inst(LD_8_TO_FF_IMMEDIATE(0x40));

    // Tile 0 = 0xFF bytes (white = dead)
    asm.inst(LD_16_IMMEDIATE(HL, 0x8000))
        .inst(LD_8_IMMEDIATE(B, 16));
    asm.label("TILE0")
        .inst(LD_8_IMMEDIATE(A, 0xFF))
        .inst(LD_8_TO_SECONDARY(AT_HL_Plus))
        .inst(DEC(B))
        .jr_cond(if_NZ, "TILE0");

    // Tile 1 = 0x00 bytes (black = alive)
    asm.inst(LD_8_IMMEDIATE(B, 16));
    asm.label("TILE1")
        .inst(LD_8_IMMEDIATE(A, 0x00))
        .inst(LD_8_TO_SECONDARY(AT_HL_Plus))
        .inst(DEC(B))
        .jr_cond(if_NZ, "TILE1");

    // Fill entire BG map with tile 0 (white)
    asm.inst(LD_16_IMMEDIATE(HL, 0x9800))
        .inst(LD_16_IMMEDIATE(BC, 32 * 32));
    asm.label("CLR_BG")
        .inst(LD_8_IMMEDIATE(A, 0))
        .inst(LD_8_TO_SECONDARY(AT_HL_Plus))
        .inst(DEC_16(BC))
        .inst(LD_8_INTERNAL(A, B))
        .inst(OR(C))
        .jr_cond(if_NZ, "CLR_BG");

    // Initialize 1D grid: cells 3,4 alive (0b00011000)
    for i in 0..CELLS {
        let val: u8 = if i == 3 || i == 4 { 1 } else { 0 };
        asm.inst(LD_16_IMMEDIATE(HL, ROW_BASE + i as u16))
            .inst(LD_8_IMMEDIATE(A, val))
            .inst(LD_8_INTERNAL(AT_HL, A));
    }

    // write_row = 0 (start writing at row 0)
    asm.inst(LD_16_IMMEDIATE(HL, WRITE_ROW))
        .inst(LD_8_IMMEDIATE(A, 0))
        .inst(LD_8_INTERNAL(AT_HL, A));

    // ===== MAIN LOOP =====
    asm.label("MAIN");

    // --- RENDER current generation to BG row[write_row] ---
    // We need to write to bg_addr(write_row, col) for col 0..7
    // write_row is at WRITE_ROW; we need to compute the BG address.
    //
    // bg_addr(row, 0) = 0x9800 + (TILE_ROW + row) * 32 + TILE_COL
    //                 = 0x9800 + TILE_ROW*32 + TILE_COL + row*32
    //                 = BASE_ADDR + row * 32
    // where BASE_ADDR = 0x9800 + TILE_ROW*32 + TILE_COL
    let bg_base: u16 = 0x9800 + TILE_ROW * BG_COLS + TILE_COL;

    // Load write_row into A, multiply by 32 (= shift left 5), add bg_base → HL
    asm.inst(LD_8_FROM_MEMORY_IMMEDIATE(WRITE_ROW))  // A = write_row
        .inst(LD_16_IMMEDIATE(HL, bg_base));          // HL = bg_base

    // HL += A * 32: shift A left 5 times, add to HL
    // We'll use: add HL, (A as HL). Encode A*32 by repeated doubling.
    // A*32 = A<<5. Since A ≤ 7, A*32 ≤ 224, fits in u8.
    // Trick: put A into L, shift HL left 5 (= multiply HL by 32 / shift L)
    // Simpler: use DE = A, then add HL 5 times (shift-double DE).
    // Actually cleanest: compute A*32 in B, then ADD_TO_HL with BC.
    //   A * 32: since A ≤ 7, use RLCA 5 times (rotate A left 5, clear low bits)
    //   But RLCA rotates through carry. Let's just use A*2 five times:
    asm.inst(ADD(A));  // A *= 2
    asm.inst(ADD(A));  // A *= 4
    asm.inst(ADD(A));  // A *= 8
    asm.inst(ADD(A));  // A *= 16
    asm.inst(ADD(A));  // A *= 32
    // Now A = write_row * 32. Load into C, B=0, then ADD_TO_HL BC
    asm.inst(LD_8_INTERNAL(C, A))
        .inst(LD_8_IMMEDIATE(B, 0))
        .inst(ADD_TO_HL(U16Register::BC));
    // HL now points to the leftmost tile of our target BG row

    // Write 8 cells from ROW_BASE to BG
    asm.inst(LD_16_IMMEDIATE(DE, ROW_BASE))
        .inst(LD_8_IMMEDIATE(C, CELLS as u8));
    asm.label("RENDER_CELL")
        .inst(LD_8_FROM_SECONDARY(AT_DE))
        .inst(INC_16(DE))
        .inst(LD_8_INTERNAL(AT_HL, A))
        .inst(INC_16(HL))
        .inst(DEC(C))
        .jr_cond(if_NZ, "RENDER_CELL");

    // --- WAIT for vblank (so display is stable) ---
    // Poll LY register (0xFF44) until it reaches 144
    asm.label("VBLANK_WAIT")
        .inst(LD_8_FROM_FF_IMMEDIATE(0x44))  // A = LY
        .inst(CP_IMMEDIATE(144))
        .jr_cond(if_NZ, "VBLANK_WAIT");

    // --- EVOLVE: compute next generation ---
    // For each cell i (0..7): neighbors are (i-1) mod 8 and (i+1) mod 8
    // count = left + right
    // next = if count==2 { 1 } else if count==1 { alive } else { 0 }
    // Write result to scratch at 0xC020..0xC027
    let scratch: u16 = 0xC020;

    for i in 0..CELLS {
        let left  = ROW_BASE + ((i + CELLS - 1) % CELLS) as u16;
        let right = ROW_BASE + ((i + 1) % CELLS) as u16;
        let cur   = ROW_BASE + i as u16;
        let dst   = scratch + i as u16;

        let lbl_born   = format!("BORN_{}", i);
        let lbl_check2 = format!("CHK2_{}", i);
        let lbl_die    = format!("DIE_{}", i);
        let lbl_end    = format!("END_{}", i);

        // count = left + right
        asm.inst(LD_8_FROM_MEMORY_IMMEDIATE(left))
            .inst(LD_16_IMMEDIATE(HL, right))
            .inst(ADD(AT_HL));                      // A = left + right (count)

        // if count == 2: born/survive (write 1)
        asm.inst(CP_IMMEDIATE(2));
        asm.jp_cond(if_Z, &lbl_born);

        // if count == 1: survive only if alive
        asm.inst(CP_IMMEDIATE(1));
        asm.jp_cond(if_NZ, &lbl_die);              // count == 0 → die

        // count == 1: check alive
        asm.label(&lbl_check2);
        asm.inst(LD_8_FROM_MEMORY_IMMEDIATE(cur)); // A = current state
        asm.inst(CP_IMMEDIATE(1));
        asm.jp_cond(if_NZ, &lbl_die);             // dead with 1 neighbor → stays dead

        // survive (count=1, alive=1)
        asm.label(&lbl_born);
        asm.inst(LD_16_IMMEDIATE(HL, dst))
            .inst(LD_8_IMMEDIATE(A, 1))
            .inst(LD_8_INTERNAL(AT_HL, A));
        asm.jp(&lbl_end);

        asm.label(&lbl_die);
        asm.inst(LD_16_IMMEDIATE(HL, dst))
            .inst(LD_8_IMMEDIATE(A, 0))
            .inst(LD_8_INTERNAL(AT_HL, A));

        asm.label(&lbl_end);
    }

    // Copy scratch → ROW_BASE
    asm.inst(LD_16_IMMEDIATE(HL, ROW_BASE))
        .inst(LD_16_IMMEDIATE(DE, scratch))
        .inst(LD_8_IMMEDIATE(C, CELLS as u8));
    asm.label("COPY")
        .inst(LD_8_FROM_SECONDARY(AT_DE))
        .inst(INC_16(DE))
        .inst(LD_8_INTERNAL(AT_HL, A))
        .inst(INC_16(HL))
        .inst(DEC(C))
        .jr_cond(if_NZ, "COPY");

    // --- ADVANCE write_row = (write_row + 1) % 8 ---
    asm.inst(LD_8_FROM_MEMORY_IMMEDIATE(WRITE_ROW))
        .inst(INC(A))
        .inst(AND_IMMEDIATE(0x07))               // mod 8
        .inst(LD_16_IMMEDIATE(HL, WRITE_ROW))
        .inst(LD_8_INTERNAL(AT_HL, A));

    // --- DELAY: burn some cycles so it's not too fast ---
    // Nested loop: outer * inner iterations
    asm.inst(LD_8_IMMEDIATE(B, 8));
    asm.label("DELAY_OUT")
        .inst(LD_8_IMMEDIATE(C, 0)); // 256 inner iters
    asm.label("DELAY_IN")
        .inst(DEC(C))
        .jr_cond(if_NZ, "DELAY_IN")
        .inst(DEC(B))
        .jr_cond(if_NZ, "DELAY_OUT");

    asm.jp("MAIN");

    // ===== ASSEMBLE =====
    let instructions = asm.assemble();
    let code_bytes: Vec<u8> = instructions.iter()
        .flat_map(|i| i.clone().to_bytes())
        .collect();

    let mut rom = vec![0u8; 32768];

    // Minimal header
    rom[0x0100] = 0x00; // NOP
    rom[0x0101] = 0xC3; // JP
    rom[0x0102] = 0x50;
    rom[0x0103] = 0x01; // 0x0150

    // Nintendo logo (required for boot)
    let logo: [u8; 48] = [
        0xCE,0xED,0x66,0x66,0xCC,0x0D,0x00,0x0B,
        0x03,0x73,0x00,0x83,0x00,0x0C,0x00,0x0D,
        0x00,0x08,0x11,0x1F,0x88,0x89,0x00,0x0E,
        0xDC,0xCC,0x6E,0xE6,0xDD,0xDD,0xD9,0x99,
        0xBB,0xBB,0x67,0x63,0x6E,0x0E,0xEC,0xCC,
        0xDD,0xDC,0x99,0x9F,0xBB,0xB9,0x33,0x3E,
    ];
    rom[0x0104..0x0134].copy_from_slice(&logo);

    // Title
    for (i, b) in b"LIFE1D      ".iter().enumerate() {
        rom[0x0134 + i] = *b;
    }
    rom[0x0147] = 0x00; // ROM only
    rom[0x0148] = 0x00; // 32KB
    rom[0x0149] = 0x00; // no RAM

    // Header checksum
    let checksum: u8 = rom[0x0134..=0x014C]
        .iter()
        .fold(0u8, |acc, &b| acc.wrapping_sub(b).wrapping_sub(1));
    rom[0x014D] = checksum;

    // Code at 0x0150
    for (i, b) in code_bytes.iter().enumerate() {
        if 0x0150 + i < rom.len() {
            rom[0x0150 + i] = *b;
        }
    }

    println!("Code size: {} bytes", code_bytes.len());
    rom
}
