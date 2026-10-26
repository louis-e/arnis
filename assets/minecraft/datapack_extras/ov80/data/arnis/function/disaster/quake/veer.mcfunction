# Cracks wander a little instead of running straight.
execute store result score #r arnis.ds run random value 1..3
execute if score #r arnis.ds matches 1 run rotate @s ~-14 ~
execute if score #r arnis.ds matches 3 run rotate @s ~14 ~
