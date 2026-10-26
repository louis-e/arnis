execute if score @s arnis.ds.ctl matches 1 run function arnis:disaster/start
execute if score @s arnis.ds.ctl matches 2 if score #state arnis.ds matches 1 if score #phase arnis.ds matches 1 run scoreboard players set #t arnis.ds 1
execute if score @s arnis.ds.ctl matches 3 if score #state arnis.ds matches 1 run function arnis:disaster/stop
scoreboard players reset @s arnis.ds.ctl
