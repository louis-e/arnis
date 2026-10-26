scoreboard players add #cd arnis.ds 1
execute if score #cd arnis.ds >= #rate arnis.ds run function arnis:disaster/storm/volley
execute as @e[type=marker,tag=arnis.ds.bolt] at @s run function arnis:disaster/storm/bolt
