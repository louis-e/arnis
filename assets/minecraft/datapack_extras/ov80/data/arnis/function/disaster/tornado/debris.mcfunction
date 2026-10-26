# Held at the funnel and turned with it; the climb and the tumble are one interpolation.
execute rotated as @s run tp @s ~ ~ ~ ~24 0
scoreboard players add @s arnis.ds.t 1
execute if score @s arnis.ds.t matches 2 run data merge entity @s {start_interpolation:0,interpolation_duration:56,transformation:{left_rotation:[0.38f,0.6f,0.2f,0.67f],right_rotation:[0.0f,0.0f,0.0f,1.0f],translation:[8.0f,30.0f,0.0f],scale:[0.8f,0.8f,0.8f]}}
execute if score @s arnis.ds.t matches 58.. run function arnis:disaster/tornado/debris_end
