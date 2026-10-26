tag @s add arnis.ds.inside
effect give @s minecraft:levitation 1 3 true
execute at @s run rotate @s ~16 ~
scoreboard players add @s arnis.ds.t 1
scoreboard players operation #m arnis.ds = @s arnis.ds.t
scoreboard players operation #m arnis.ds %= #10 arnis
execute if score #m arnis.ds matches 0 run damage @s 1 minecraft:generic
execute if score @s arnis.ds.t matches 40.. at @s run function arnis:disaster/tornado/fling
