## Path handling and Unicode safety

Currently we use Rust `String` for all paths, but internally treat paths as
bytes, including using "unsafe" sometimes to treat paths as bytes.

Based on my superficial understanding of how safety relates to UTF8 in Rust
strings, it's probably harmless given that we never treat strings as Unicode,
but it's also possible some code outside of our control relies on this.

The proper fix is to switch to a bag of bytes type.  I attempted this initially
but ran into trouble making my custom string type compatible with hash tables.
