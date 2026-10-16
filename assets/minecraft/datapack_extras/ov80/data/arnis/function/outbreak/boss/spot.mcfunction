execute store result entity @s Rotation[0] float 1 run random value 0..359
execute at @s run tp @s ^ ^ ^32
execute at @s run spreadplayers ~ ~ 0 16 false @s
execute at @s run function arnis:outbreak/boss/rise
kill @s
