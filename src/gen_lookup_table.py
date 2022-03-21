#!/usr/bin/env python3

# See parse.rs: is_ident_char().

ident_chars = ['az', 'AZ', '09', '_', '-', '.']
path_chars = ['az', 'AZ', '09', '_', '-', '.', '/', ',', '+']
chars = path_chars

tab = [0 for _ in range(256)]
for span in path_chars:
  if len(span) > 1:
    for c in range(ord(span[0]), ord(span[1])+1):
      tab[c] = 1
  else:
    tab[ord(span)] = 1

for ofs in range(0, 256, 64):
  bits = tab[ofs:ofs+64]
  s = ''.join('1' if b else '0' for b in bits)[::-1]
  num = int(s, 2)
  print(hex(num))