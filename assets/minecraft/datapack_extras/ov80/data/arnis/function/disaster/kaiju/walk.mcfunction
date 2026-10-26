execute if score #pass arnis.ds matches ..1 rotated as @s rotated ~ 0 run tp @s ^ ^ ^0.17
execute if score #pass arnis.ds matches 2.. rotated as @s rotated ~ 0 run tp @s ^ ^ ^0.2
scoreboard players operation #m arnis.ds = #kj_tk arnis.ds
scoreboard players operation #m arnis.ds %= #4 arnis
execute if score #m arnis.ds matches 0 run function arnis:disaster/kaiju/smash
scoreboard players operation #m arnis.ds = #kj_tk arnis.ds
scoreboard players operation #m arnis.ds %= #10 arnis
execute if score #m arnis.ds matches 0 run function arnis:disaster/util/follow_ground
scoreboard players operation #m arnis.ds = #kj_tk arnis.ds
scoreboard players operation #m arnis.ds %= #16 arnis
execute if score #m arnis.ds matches 0 run function arnis:disaster/kaiju/step
execute if score #m arnis.ds matches 2 as @a[distance=..40,gamemode=!spectator] at @s run rotate @s ~ ~-1.2
scoreboard players operation #m arnis.ds = #kj_tk arnis.ds
scoreboard players operation #m arnis.ds %= #40 arnis
execute if score #m arnis.ds matches 0 run function arnis:disaster/kaiju/steer
execute if score #kj_boom arnis.ds matches 1.. run function arnis:disaster/kaiju/charge
