# Week 1

## Day 1
One mutable memtable, followed by zero or more immutable memtables awaiting flush.

**Invariants:**
- The engine has exaclty one current mutable memtable.
- The `imm_memtables` are ordered from newest to oldest.
- The first version of the key found while probing from newest to oldest determines the results.
- An empty value is a tombstone, that determines that the key is absent; the search must not continue to older values. 
  In mini-lsm, a key associated with an empty value represents a deletion.
- Structural changes such as replacing the mutable memtable is serialized, even though reads and writes to the mutable memtable are concurrent.

We are not using `upgradable_read` because all put operations will require a read lock but upgradable read can only be granted to one thread
at a time, so in this particular use case, because we want to allow all threads to perform put operations without synchorinizing them via 
locks, upgradable read would be the wrong choice, as it'll work like a write lock rather than a read lock.

## Day 2
The output is sorted, and each user key appears at most once with the value from the newest input containing that key. Tombstones participate
in the precedence before `LsmIterator` hides them.

**Invariants:**
- `MemTableIterator` exposes one sorted memtable.
- `MergeIterator` combines sources of the same type, removes duplicate versions, and gives lower-indexed—that is, newer—sources precedence.
- `LsmIterator` removes tombstones from the user-visible stream.
- `FusedIterator` makes invalid and errored states safe for callers.

Uses a cursor based API, common in databases and inspired by RocksDB's iterator.

## Day 3
Blocks: A compact sequence of sorted entries plus an index of their starting offsets.

- Encoding and Decoding are inverses for every valid block.
- Entry offsets are ordered and refer to valid positions in the data section.
- The footer contains exactly one offset per entry and the encoded entry count.
- Except when the first entry alone exceed the target, adding an entry must not make the block larger than `target_size`.
- Seeking, positions the iterator at the first key greater than or equal to the target, or makes it invalid if no such key exists.

An SST consists of multiple blocks, and when enough immutable memtables accumumlate, the engine flushes the oldest one to an SST.

The block encoding format:
```
----------------------------------------------------------------------------------------------------
|             Data Section             |              Offset Section             |      Extra      |
----------------------------------------------------------------------------------------------------
| Entry #1 | Entry #2 | ... | Entry #N | Offset #1 | Offset #2 | ... | Offset #N | num_of_elements |
----------------------------------------------------------------------------------------------------
```

Each `Entry` is a key value pair:
```
-----------------------------------------------------------------------
|                           Entry #1                            | ... |
-----------------------------------------------------------------------
| key_len (2B) | key (keylen) | value_len (2B) | value (varlen) | ... |
-----------------------------------------------------------------------
```

## Day 4
An SST composes many blocks into an immutable file and adds enough metadata to locate which block the requested key belongs to.

**Invariants:**
- Blocks and their metadata appear in the same key order.
- Each `BlockMeta` records block's offset and their actual first and last key.
- An iterator directly retains only the block it currently needs and will move to the next one if a seek lands beyond the current
  block. The cache may retain other blocks independently.
- A lower bound key never skips a key greater than or equal to the target.
- The block cache identifies a block both by its SST_ID's and block index. Block from different SSDs must never collide.
