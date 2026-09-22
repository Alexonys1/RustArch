use std::cmp::Ordering;
use std::collections::BinaryHeap;

use super::core::ALPHABET_SIZE;


pub fn build_tree(nonzero_freqs: &[SymbolWithFreq]) -> NodeOfHuffmanTree {
    let mut heap: BinaryHeap<HeapEntry> = BinaryHeap::with_capacity(nonzero_freqs.len());
    let mut seq: u64 = 0; // Дополнительный порядок нужен для объединений листьев.
    // До этого у нас был алфавитный порядок у символов. Но после объединения
    // он пропадает. Именно поэтому сравнение листьев происходит через частоты, а потом seq.

    for &SymbolWithFreq { symbol, freq } in nonzero_freqs.iter() {
        heap.push(HeapEntry { node: NodeOfHuffmanTree::Leaf { symbol, freq }, seq });
        seq += 1;
    }

    while heap.len() > 1 {
        let a: HeapEntry = heap.pop().unwrap();
        let b: HeapEntry = heap.pop().unwrap();

        let merged = NodeOfHuffmanTree::Node {
            freq: a.node.freq() + b.node.freq(),
            left: Box::new(a.node),
            right: Box::new(b.node),
        };

        heap.push(HeapEntry { node: merged, seq });
        seq += 1;
    }

    // Возвращаем корень:
    heap
        .pop()
        .expect("build_tree вызван с пустым списком символов!")
        .node
}


/// Заполняет таблицу кодов, накапливая биты сдвигами прямо в `u32` по ходу рекурсии.
pub fn create_huffman_codes(root: NodeOfHuffmanTree) -> [HuffmanCode; ALPHABET_SIZE] { // Вроде компилятор это оптимизирует
    let mut codes = [HuffmanCode::default(); ALPHABET_SIZE];
    assign_codes(&root, 0, 0, &mut codes);
    codes
}

fn assign_codes(node: &NodeOfHuffmanTree, code: u32, len: u8, codes: &mut [HuffmanCode]) { // TODO: А насколько длинные коды там могут быть?
    match node {
        NodeOfHuffmanTree::Leaf { symbol, .. } => {
            codes[*symbol as usize] = HuffmanCode { code, len };
        }
        NodeOfHuffmanTree::Node { left, right, .. } => {
            assign_codes(left, code << 1, len + 1, codes);        // Переход влево добавляет бит 0
            assign_codes(right, (code << 1) | 1, len + 1, codes); // Переход вправо добавляет бит 1
        }
    }
}


/// Конечно, можно держать код переменной длины в виде вектора булов.
/// Но такой подход показал свою неэффективность из-за сликом большого числа косвенных обращений к памяти.
/// + нужно было всё равно склеивать биты...
/// Поэтому я принял решение сделать все коды фиксированный длины с доп.
/// полем длины полезных битов (не все же 32 бита это код).
/// Это дало прирост в скорости чуть ли не в 2 раза!
#[derive(Copy, Clone, Default)]
pub struct HuffmanCode {
    pub code: u32,
    pub len: u8,
}

#[derive(Copy, Clone)]
pub struct SymbolWithFreq {
    pub symbol: u8,
    pub freq: u64,
}

pub enum NodeOfHuffmanTree {
    Leaf { symbol: u8, freq: u64 },
    Node { freq: u64, left: Box<NodeOfHuffmanTree>, right: Box<NodeOfHuffmanTree> },
}

impl NodeOfHuffmanTree {
    pub fn freq(&self) -> u64 {
        match self {
            NodeOfHuffmanTree::Leaf { freq, .. } => *freq,
            NodeOfHuffmanTree::Node { freq, .. } => *freq,
        }
    }
}


struct HeapEntry {
    node: NodeOfHuffmanTree,
    seq: u64,
}

impl PartialEq for HeapEntry {
    fn eq(&self, other: &Self) -> bool {
        self.node.freq() == other.node.freq() && self.seq == other.seq
    }
}

impl Eq for HeapEntry {}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering { // Сравнение развёрнуто!! Листья дерева минимальны, а не максимальны.
        other.node.freq().cmp(&self.node.freq()) // Корень максимален.
            .then_with(|| other.seq.cmp(&self.seq))
    }
}
