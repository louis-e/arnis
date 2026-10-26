tp @s ~ ~-0.4 ~
execute positioned over ocean_floor run particle minecraft:block{block_state:"minecraft:dirt"} ~ ~0.5 ~ 5 1 5 0 30 force
execute store result score #cy arnis.ds run data get entity @s Pos[1]
scoreboard players operation #cy arnis.ds -= #kj_gy arnis.ds
execute if score #cy arnis.ds matches ..-30 run function arnis:disaster/kaiju/gone
