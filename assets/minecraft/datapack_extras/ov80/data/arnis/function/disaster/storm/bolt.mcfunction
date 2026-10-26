particle minecraft:electric_spark ~ ~0.6 ~ 0.4 1.2 0.4 0.05 5 force
scoreboard players remove @s arnis.ds.t 1
execute if score @s arnis.ds.t matches ..0 run summon lightning_bolt ~ ~ ~
execute if score @s arnis.ds.t matches ..0 run kill @s
