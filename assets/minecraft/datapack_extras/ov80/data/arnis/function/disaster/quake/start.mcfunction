scoreboard players set #len arnis.ds 25
execute if score #pass arnis.ds matches 2.. run scoreboard players set #len arnis.ds 35
scoreboard players set #qk_next arnis.ds 2
execute in minecraft:overworld as @a[distance=0..] at @s run playsound minecraft:entity.warden.emerge ambient @s ~ ~-4 ~ 2 0.5
