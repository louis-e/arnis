# The edge crumbles a little before it goes.
scoreboard players remove @s arnis.ds.t 1
execute if score @s arnis.ds.t matches 8 if score @s arnis.ds.id matches ..7 run particle minecraft:falling_dust{block_state:"minecraft:dirt"} ~ ~0.5 ~ 5 0.3 5 0 20 force
execute if score @s arnis.ds.t matches 8 if score @s arnis.ds.id matches 8.. run particle minecraft:falling_dust{block_state:"minecraft:dirt"} ~ ~0.5 ~ 9 0.3 9 0 40 force
execute if score @s arnis.ds.t matches 8 run playsound minecraft:block.rooted_dirt.break ambient @a[distance=..40] ~ ~ ~ 1.5 0.5
execute if score @s arnis.ds.t matches 1.. run return 0
function arnis:disaster/sinkhole/grow
