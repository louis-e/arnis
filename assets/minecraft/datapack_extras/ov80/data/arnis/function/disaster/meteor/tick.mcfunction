scoreboard players add #cd arnis.ds 1
execute if score #cd arnis.ds >= #rate arnis.ds run function arnis:disaster/meteor/volley
execute as @e[type=block_display,tag=arnis.ds.rock] at @s run function arnis:disaster/meteor/fly
execute as @e[type=marker,tag=arnis.ds.impact] at @s run function arnis:disaster/meteor/mark
execute as @e[type=marker,tag=arnis.ds.crater] at @s run function arnis:disaster/meteor/settle
