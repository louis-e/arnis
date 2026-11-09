scoreboard players add @s arnis.wb 1
# Held up for ten seconds by a player who stays put: go back the other way.
execute if score @s arnis.wb matches 100.. run function arnis:walk/turn_back
