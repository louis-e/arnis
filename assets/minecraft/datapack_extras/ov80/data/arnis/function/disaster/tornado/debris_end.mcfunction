execute store result score #r arnis.ds run random value 1..3
execute if score #r arnis.ds matches 1 if entity @s[tag=arnis.ds.leaf] run function arnis:disaster/tornado/throw
kill @s
