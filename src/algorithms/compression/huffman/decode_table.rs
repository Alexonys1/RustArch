use super::core::{TABLE_BITS, TABLE_SIZE};
use super::tree_bulder::NodeOfHuffmanTree;


#[derive(Clone, Copy)]
pub enum DecodeEntry {
    Symbol { symbol: u8, code_len: u8 },
    Escape,
}


pub fn build_decode_table(tree: &NodeOfHuffmanTree) -> [DecodeEntry; TABLE_SIZE] {
    let mut table = [DecodeEntry::Escape; TABLE_SIZE];
    fill_table(tree, 0, 0, &mut table);
    table
}


fn fill_table(node: &NodeOfHuffmanTree, code: u32, code_len: u32, table: &mut [DecodeEntry]) {
    match node {
        NodeOfHuffmanTree::Leaf { symbol, .. } => {
            if code_len <= TABLE_BITS {
                let shift = TABLE_BITS - code_len;
                let base = (code as usize) << shift;
                
                for i in 0..(1usize << shift) {
                    table[base + i] = DecodeEntry::Symbol { symbol: *symbol, code_len: code_len as u8 };
                }
            }
            // len > TABLE_BITS: соответствующий узел не укладывается в
            // таблицу - все ведущие к нему индексы остаются Escape
            // (не были и не будут перезаписаны), декодер пойдёт по дереву.
        }

        NodeOfHuffmanTree::Node { left, right, .. } => {
            if code_len < TABLE_BITS {
                fill_table(left, code << 1, code_len + 1, table);
                fill_table(right, (code << 1) | 1, code_len + 1, table);
            }
            // len == TABLE_BITS и это внутренний узел (код ещё не
            // разрешился в символ за отведённые биты) - глубже не идём,
            // это и есть Escape-случай для длинных кодов.
        }
    }
}
