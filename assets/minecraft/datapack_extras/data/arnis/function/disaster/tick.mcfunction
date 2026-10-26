# The disasters need 1.21.6; the real ones live in the ov80 overlay.
execute as @a unless score @s arnis.ds.seen matches 1.. run function arnis:disaster/unsupported
