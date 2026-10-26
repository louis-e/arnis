execute store result score #r arnis.ds run random value 1..3
execute if score #r arnis.ds matches 1 if data entity @s block_state{Name:"minecraft:oak_leaves"} run function arnis:disaster/tornado/throw
kill @s
