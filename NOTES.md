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

**Task1:**

An SST consists of data blocks and index blocks, they are loaded lazily when being read. We don't implement the index blocks and
assume that for our usecase the entire `BlockMeta` fits within the main memory.

The `BlockMeta` must contain the information for every block and concerning its `offset`, `first_key`, and `last_key`.

The SST encoding has the following layout.
```
-------------------------------------------------------------------------------------------
|         Block Section         |          Meta Section         |          Extra          |
-------------------------------------------------------------------------------------------
| data block | ... | data block |            metadata           | meta block offset (u32) |
-------------------------------------------------------------------------------------------
```

## Day 5

![week1-day5 overview](assets/week1-05-overview.svg)

**Read Path Invariants**

- Mutable and Immutable memtables take precendence over SSTables, and newer sources take precedence over older sources within each group.
- A tombstone in a newer source hides every other older value for that key.
- `scan` returns sorted, unique, live values within the exact requested bounds.
- A seek may land on the next greater key, so `get` returns a value only after checking for exact key equality.
- Creating and Seeking SST iterators may perform I/O and therefore happens after releasing the state lock, using a consistent cloned
  snapshot

## Day 6

![week1-day5 overview](assets/week1-05-overview.svg)

**Write Path Invariants**

- Only the oldest immutable memtable is selected for the next flush.
- `state_lock` serializes the flush with other structural changes.
- Building and writing the SST occurs outside the read-write lock.
- Installing the SST removes exactly the memtable that was flushed, inserts the `SsTable` into `ss_tables`, and inserts its ID at the 
  front - the newest side - of the `l0_ss_tables` in one snapshot update.
- SST optimization may only exclude a file when its key range cannot contribute to the user request. An optimization should not change
  query results.

## Day 7

![week1-day7 overview](assets/week1-07-overview.svg)

**Optimization Invariants**

- A Bloom Filter negative result must be deterministic: An SST skipped after a negative probe cannot contain the key at all, but a positive
  result may be false and therefore still require a lookup.
- The builder and reader hash exactly the same key bytes with the same function.
- Prefix encoding and then decoding reconstructs every original key exactly.
- Each compressed key is defined relative to the first key in its block, so it can be decoded without decoding every preceeding entry.
- These optimizations may change I/O counts and encoded sized, but never the key-value pairs returned from `get` or `scan`.

---

### 1. What is `nbits`?

A Bloom filter is just a **long row of switches (bits)**, where each switch is either `0` (off) or `1` (on).

**`nbits` is simply the total number of switches (bits) in our Bloom filter.**

For example:
- Suppose our SSTable has `100` keys.
- We decide to spend `10` bits of memory per key (`bits_per_key = 10`).
- Then `nbits = 100 * 10 = 1000` bits (which is `125` bytes of RAM, since $125 \times 8 = 1000$).
- The bit positions are numbered `0, 1, 2, ..., 999`.

Whenever we have a huge hash number `h` (which can be up to $4,294,967,295$), we need to pick a bit position between `0` and `999`.
How do you turn any huge number into a number between `0` and `999`?
You take the remainder: **`bit_pos = h % nbits`** (`h % 1000`).

---

### 2. What are "High Bits" and "Low Bits"? (And why `%` ignores High Bits)

Before looking at binary bits, think about normal **base-10 (decimal) digits**.

Suppose `farmhash` gives you an 8-digit random number:
`h = 84,920,317`

- The **leftmost digits (`8492`)** are the **"high" digits** (they represent tens of millions, millions, etc.).
- The **rightmost digits (`0317`)** are the **"low" digits** (they represent hundreds, tens, ones).

Now suppose `nbits = 1,000`, so you compute:
`bit_pos = h % 1,000`

What is `84,920,317 % 1,000`?
It is **`317`**!

Look at what just happened:
- `84,920,317 % 1,000 = 317`
- `11,110,317 % 1,000 = 317`
- `99,990,317 % 1,000 = 317`

The `% 1,000` operation **completely threw away the left ("high") digits `8492`** and only used the right ("low") digits `317`!

#### Now in Binary (32-bit `u32`):

A `u32` is a 32-digit binary number (32 bits of `0`s and `1`s):
```text
Bit index:  31              16 15               0
           +------------------+------------------+
      h =  | 1011010011100010 | 0001110101001011 |
           +------------------+------------------+
             ^^^^^^^^^^^^^^^    ^^^^^^^^^^^^^^^^
               "High bits"        "Low bits"
            (left half: big     (right half: small
             powers of 2)        powers of 2)
```
When `farmhash(key)` hashes a key, it works really hard to make **all 32 bits** uniformly random.
- But when you do `h % nbits` (where `nbits` is small, say `1024`, which is $2^{10}$), `h % 1024` **only looks at the rightmost 10 bits ("low bits")**!
- The leftmost 22 bits ("high bits") of `h` had good randomness in them, and `h % 1024` didn't even look at them!

---

### 3. Are We Trying to "Create" Randomness? No—We Are *Reusing* the High Bits!

You asked: *"Are we essentially trying to inject as much randomness as possible after hashing it the first time?"*

Close, but here is the key insight: **You cannot create new randomness out of thin air using bit shifts.**
Rather, `farmhash` **already gave us 32 bits of high-quality randomness** in `h`.
- A single `h % nbits` only uses the **bottom (low) bits** of `h`.
- The **top (high) bits** of `h` are sitting there unused!

So how do we bring those unused **top bits** down into the **bottom bits** so that our 2nd, 3rd, 4th, ..., $k$-th checks can use them?

#### Step A: Swap the Top and Bottom Halves (`delta`)

That is ALL `let delta = (h >> 17) | (h << 15)` does!
In our decimal analogy with `h = 8492_0317`:
- If you cut `8492_0317` in the middle and swap the left half and right half, you get:
  `delta = 0317_8492`
- Notice that the "high" digits (`8492`) that were ignored before are now at the **bottom** (rightmost position) of `delta`!

In 32-bit binary:
- `h >> 17` takes the leftmost 15 bits and moves them to the rightmost 15 bits.
- `h << 15` takes the rightmost 17 bits and moves them to the leftmost 17 bits.
- `|` (OR) glues them together into `delta`.

---

### 4. Why `h = h.wrapping_add(delta)` in a Loop?

Now we have two numbers derived from our key:
1. `h` (our starting hash)
2. `delta` (our step size, which has `h`'s high bits sitting in its low bits)

Remember we need **$k$ different bit positions** (say $k = 5$) for this one key.

How do we generate 5 different numbers using `h` and `delta`?

We just **keep adding `delta` to `h`** on each turn of the loop!

Let's trace the loop with our decimal analogy (`h = 8492_0317`, `delta = 0317_8492`, `nbits = 1000`):

- **Iteration 0:**
  - `bit_pos = h % 1000` -> `84920317 % 1000` = **`317`** (Sets bit 317)
  - `h = h + delta` -> `84920317 + 03178492 = 88098809`
- **Iteration 1:**
  - `bit_pos = h % 1000` -> `88098809 % 1000` = **`809`** (Sets bit 809)
  - `h = h + delta` -> `88098809 + 03178492 = 91277301`
- **Iteration 2:**
  - `bit_pos = h % 1000` -> `91277301 % 1000` = **`301`** (Sets bit 301)
  - `h = h + delta` -> `91277301 + 03178492 = 94455793`
- **Iteration 3:**
  - `bit_pos = h % 1000` -> `94455793 % 1000` = **`793`** (Sets bit 793)

Look at the bit positions we got: **`317`, `809`, `301`, `793`**.

Why did we get a completely different sequence of bit positions?

Because `delta` had `8492` at the bottom! Even if two different keys happened to have the **exact same low digits** `0317` 
(say `8492_0317` and `5106_0317`):

- Both keys hit bit **`317`** on Iteration 0.
- Key 1 adds its `delta` (`...8492`) and jumps to **`809`**, **`301`**, **`793`**.
- Key 2 adds its `delta` (`...5106`) and jumps to **`423`**, **`529`**, **`635`**.

Because their high bits were different, their `delta` step sizes are different, so they immediately diverge after Iteration 0!

*(And why `wrapping_add` instead of `+`? Because in Rust debug mode, if `h + delta` exceeds the max `u32` value `4,294,967,295`, normal `+` 
panics with integer overflow. `wrapping_add` tells Rust to wrap around modulo $2^{32}$, which is standard unsigned 32-bit hardware addition.)*

---

### 5. Why `17` and `15` Instead of `16` and `16`?

You don't need any number theory here—it's a very simple practical reason:

- A 32-bit integer has 32 bits. Any split that adds up to 32 (`16 + 16`, or `17 + 15`) swaps the left part and the right part without losing any bits.
- Computer memory is organized in **8-bit bytes** (`8, 16, 24, 32`).
- If you split at `16` and `16`, you are splitting right on a whole-byte boundary.
- Splitting at **`17` and `15`** ($17 + 15 = 32$) cuts right through the middle of a byte! That way, bits from byte 2 get mixed into 
  byte 0 and byte 1. The authors of Google's LevelDB picked `17` and `15` for that exact reason, and every database since (RocksDB, TiKV, Mini-LSM) 
  copied that exact line: `let delta = (h >> 17) | (h << 15);`.

---
