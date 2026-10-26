# 40 steps from 72 up and 42 out down to the mark.
tp @s ^ ^ ^2.084
particle minecraft:flame ~ ~ ~ 0.5 0.5 0.5 0.03 8 force
particle minecraft:large_smoke ~ ~ ~ 0.6 0.6 0.6 0.01 4 force
particle minecraft:lava ~ ~ ~ 0.4 0.4 0.4 0 1 force
scoreboard players remove @s arnis.ds.t 1
execute if score @s arnis.ds.t matches ..0 run kill @s
