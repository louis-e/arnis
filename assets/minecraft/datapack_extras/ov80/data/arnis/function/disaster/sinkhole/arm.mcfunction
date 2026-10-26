scoreboard players set #found arnis.ds 1
tag @s add arnis.ds.sink
scoreboard players set @s arnis.ds.t 60
# How wide it opens, and how wide it gets as its edge keeps giving way.
execute store result score @s arnis.ds.id run random value 4..6
execute store result score @s arnis.ds.m run random value 8..11
execute if score #pass arnis.ds matches 2.. store result score @s arnis.ds.id run random value 5..8
execute if score #pass arnis.ds matches 2.. store result score @s arnis.ds.m run random value 11..15
execute at @s run playsound minecraft:entity.warden.dig ambient @a[distance=..48] ~ ~ ~ 2 0.6
