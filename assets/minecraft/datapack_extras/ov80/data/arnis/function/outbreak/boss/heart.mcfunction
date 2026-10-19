scoreboard players set #riding arnis.ob 0
execute on vehicle run scoreboard players set #riding arnis.ob 1
execute if score #riding arnis.ob matches 0 run function arnis:outbreak/victory
