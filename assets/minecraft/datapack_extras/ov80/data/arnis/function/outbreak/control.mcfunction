execute if score @s arnis.ob.ctl matches 1 run function arnis:outbreak/start
execute if score @s arnis.ob.ctl matches 2 if score #state arnis.ob matches 1 if score #phase arnis.ob matches 1 run time set 12900
execute if score @s arnis.ob.ctl matches 3 run function arnis:outbreak/stop
scoreboard players reset @s arnis.ob.ctl
