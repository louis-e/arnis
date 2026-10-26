scoreboard players set #len arnis.ds 55
execute if score #pass arnis.ds matches 2.. run scoreboard players set #len arnis.ds 70
execute as @e[type=marker,tag=arnis.ds.front] at @s run function arnis:disaster/wildfire/burn
execute in minecraft:overworld as @a[distance=0..] at @s run playsound minecraft:item.firecharge.use ambient @s ~ ~ ~ 1 0.6
