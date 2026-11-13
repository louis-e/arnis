# The walker ahead: their pace, how far their heading turns from the one behind's, and
# whether to wait on them: not if they are held up and younger, maybe by the one behind.
scoreboard players operation #lead arnis.walk = @s arnis.wv
scoreboard players operation #turn arnis.walk = @s arnis.wy
scoreboard players operation #turn arnis.walk -= #yaw arnis.walk
scoreboard players add #turn arnis.walk 540
scoreboard players operation #turn arnis.walk %= #360 arnis.walk
scoreboard players remove #turn arnis.walk 180
scoreboard players set #free arnis.walk 1
execute if score @s arnis.wb matches 1.. unless score @s arnis.wk < #me arnis.walk run scoreboard players set #free arnis.walk 0
