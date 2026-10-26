scoreboard players remove @s arnis.ds.t 1
execute if score @s arnis.ds.t matches ..0 run return run function arnis:disaster/meteor/impact
scoreboard players operation #m arnis.ds = @s arnis.ds.t
scoreboard players operation #m arnis.ds %= #4 arnis
execute if score #m arnis.ds matches 0 unless entity @s[tag=arnis.ds.big] run function arnis:disaster/meteor/ring
execute if score #m arnis.ds matches 0 if entity @s[tag=arnis.ds.big] run function arnis:disaster/meteor/ring_big
