execute store result score #r arnis.ds run random value 1..4
execute if score #r arnis.ds matches 1 run playsound minecraft:block.stone.hit weather @s ~3 ~2 ~-2 0.5 1.8
execute if score #r arnis.ds matches 2 run playsound minecraft:block.glass.hit weather @s ~-2 ~2 ~3 0.5 1.6
execute if score #r arnis.ds matches 3 run playsound minecraft:block.stone.hit weather @s ~-3 ~3 ~-1 0.5 2.0
execute if score #r arnis.ds matches 4 run playsound minecraft:block.wood.hit weather @s ~1 ~3 ~3 0.5 1.7
