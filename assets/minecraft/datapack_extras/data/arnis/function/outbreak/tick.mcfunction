# The outbreak needs 1.21.6; the real one lives in the ov80 overlay.
execute as @a unless score @s arnis.ob.seen matches 1.. run function arnis:outbreak/unsupported
