tp @s ~ ~0.5 ~
execute positioned over ocean_floor run particle minecraft:block{block_state:"minecraft:dirt"} ~ ~0.5 ~ 5 1 5 0 30 force
execute positioned over ocean_floor run particle minecraft:campfire_cosy_smoke ~ ~1 ~ 4 1 4 0.02 6 force
execute if score #kj_tk arnis.ds matches 1 run playsound minecraft:entity.warden.emerge hostile @a[distance=..128] ~ ~28 ~ 6 0.5
execute store result score #cy arnis.ds run data get entity @s Pos[1]
execute if score #cy arnis.ds >= #kj_gy arnis.ds run function arnis:disaster/kaiju/risen
