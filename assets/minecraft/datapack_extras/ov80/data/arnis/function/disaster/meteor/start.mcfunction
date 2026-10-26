scoreboard players set #len arnis.ds 50
scoreboard players set #rate arnis.ds 16
execute if score #pass arnis.ds matches 2.. run scoreboard players set #len arnis.ds 60
execute if score #pass arnis.ds matches 2.. run scoreboard players set #rate arnis.ds 10
# The whole shower comes in from one side of the sky.
execute store result storage arnis:ds mt.yaw int 1 run random value 0..359
