scoreboard players remove @s arnis.ds.t 1
execute if score @s arnis.ds.t matches ..0 run return run function arnis:disaster/sinkhole/drop
execute if score @s arnis.ds.id matches ..5 run particle minecraft:block{block_state:"minecraft:coarse_dirt"} ~ ~0.1 ~ 2.5 0.05 2.5 0 6 force
execute if score @s arnis.ds.id matches 6.. run particle minecraft:block{block_state:"minecraft:coarse_dirt"} ~ ~0.1 ~ 4 0.05 4 0 9 force
particle minecraft:falling_dust{block_state:"minecraft:gravel"} ~ ~1.5 ~ 2 0.3 2 0 2 force
execute if score @s arnis.ds.t matches 30 run playsound minecraft:block.rooted_dirt.break ambient @a[distance=..32] ~ ~ ~ 1.5 0.5
execute if score @s arnis.ds.t matches 12 run playsound minecraft:block.gravel.break ambient @a[distance=..32] ~ ~ ~ 1.5 0.5
