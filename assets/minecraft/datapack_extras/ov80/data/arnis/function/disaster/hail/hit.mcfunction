function arnis:disaster/util/outdoors
execute if score #out arnis.ds matches 0 run return 0
execute store result score #r arnis.ds run random value 1..2
execute if score #r arnis.ds matches 1 if score #pass arnis.ds matches ..1 run damage @s 1 minecraft:falling_block
execute if score #r arnis.ds matches 1 if score #pass arnis.ds matches 2.. run damage @s 2 minecraft:falling_block
