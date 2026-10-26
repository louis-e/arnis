scoreboard players add #cd arnis.ds 1
execute if score #cd arnis.ds matches 40.. in minecraft:overworld as @a[distance=0..] at @s run playsound minecraft:entity.player.splash.high_speed ambient @s ~ ~ ~ 1 0.5
execute if score #cd arnis.ds matches 40.. run scoreboard players set #cd arnis.ds 0
