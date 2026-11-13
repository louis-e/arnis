$execute store result score #n arnis.car run data get storage arnis:cars colours.$(kind)
execute store result score #r arnis.car run random value 0..9999
scoreboard players operation #r arnis.car %= #n arnis.car
execute store result storage arnis:cars spawn.k int 1 run scoreboard players get #r arnis.car
