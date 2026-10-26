# Now and then the bolt is for someone standing in the open.
execute store result score #r arnis.ds run random value 1..6
execute if score #r arnis.ds matches 1 run function arnis:disaster/util/outdoors
execute if score #r arnis.ds matches 1 if score #out arnis.ds matches 1 run return run execute summon marker run function arnis:disaster/storm/arm
# Otherwise it takes the highest of a few spots around them.
scoreboard players set #best arnis.ds -2147483648
execute summon marker run function arnis:disaster/storm/try
execute summon marker run function arnis:disaster/storm/try
execute summon marker run function arnis:disaster/storm/try
execute summon marker run function arnis:disaster/storm/try
execute as @e[type=marker,tag=arnis.ds.bolt_try] if score @s arnis.ds.t = #best arnis.ds run tag @s add arnis.ds.bolt_best
execute as @e[type=marker,tag=arnis.ds.bolt_best,limit=1] run function arnis:disaster/storm/arm
kill @e[type=marker,tag=arnis.ds.bolt_try]
