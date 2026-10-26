# A main crack is three wide and ten deep, a branch one wide and six.
execute if score @s arnis.ds.id matches 1 rotated as @s run fill ^-2 ^-1 ^ ^2 ^-1 ^ minecraft:coarse_dirt replace minecraft:grass_block
execute if score @s arnis.ds.id matches 1 rotated as @s run fill ^-1 ^-9 ^ ^1 ^1 ^ minecraft:air
execute if score @s arnis.ds.id matches 0 rotated as @s run fill ^ ^-5 ^ ^ ^1 ^ minecraft:air
particle minecraft:block{block_state:"minecraft:dirt"} ~ ~0.5 ~ 1 0.4 1 0 8 force
particle minecraft:falling_dust{block_state:"minecraft:gravel"} ~ ~ ~ 1 0.5 1 0 3 force
scoreboard players remove @s arnis.ds.t 1
scoreboard players operation #q arnis.ds = @s arnis.ds.t
scoreboard players operation #q arnis.ds %= #4 arnis
execute if score #q arnis.ds matches 0 run function arnis:disaster/quake/veer
execute if score #q arnis.ds matches 0 if score @s arnis.ds.id matches 1 run playsound minecraft:block.gravel.break block @a[distance=..48] ~ ~ ~ 1.5 0.6
execute if score @s arnis.ds.id matches 1 if score @s arnis.ds.t matches 15 run function arnis:disaster/quake/branch
execute if score @s arnis.ds.id matches 1 if score @s arnis.ds.t matches 30 run function arnis:disaster/quake/branch
execute rotated as @s run tp @s ^ ^ ^1
execute at @s run function arnis:disaster/util/follow_ground
execute if score @s arnis.ds.t matches ..0 run kill @s
