scoreboard players enable @a arnis.ob.ctl
execute as @a[scores={arnis.ob.ctl=1..}] at @s run function arnis:outbreak/control
scoreboard players add #ob_t arnis.ob 1
execute if score #ob_t arnis.ob matches 20.. run function arnis:outbreak/second
