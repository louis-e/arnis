scoreboard players add #cd arnis.ds 1
execute if score #cd arnis.ds matches 20.. in minecraft:overworld as @a[distance=0..] at @s run playsound minecraft:entity.warden.heartbeat hostile @s ~ ~-2 ~ 2 0.6
execute if score #cd arnis.ds matches 20.. run scoreboard players set #cd arnis.ds 0
