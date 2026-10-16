# Bright while slowing down or standing, dim while driving on.
scoreboard players set #b arnis.car 0
execute if score #c arnis.car < #was arnis.car run scoreboard players set #b arnis.car 1
execute if score #c arnis.car matches ..0 run scoreboard players set #b arnis.car 1
execute store result score #bw arnis.car run data get entity @s data.b
execute unless score #b arnis.car = #bw arnis.car run function arnis:cars/brake_switch
