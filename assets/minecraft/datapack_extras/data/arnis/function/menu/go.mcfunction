# Lands on the street beside the place; the top block there is used only if that fails. The
# places are in the Overworld wherever the player stands.
$execute in minecraft:overworld store success score #ok arnis run spreadplayers $(x) $(z) 0 1 under $(y) false @s
$execute if score #ok arnis matches 0 in minecraft:overworld run spreadplayers $(x) $(z) 0 3 false @s
